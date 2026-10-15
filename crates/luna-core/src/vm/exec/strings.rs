//! Concatenation and `tostring` conversion.

use super::*;
use crate::vm::cfmt::c_pointer;

impl Vm {
    /// Fast string concatenation of an adjacent pair, or `None` when a
    /// `__concat` metamethod is required.
    pub(super) fn concat_pair(&mut self, l: Value, r: Value) -> Result<Option<Value>, LuaError> {
        let legacy = self.float_fmt();
        // Length-check fast paths for both string operands BEFORE the
        // (expensive) copy in `concat_piece`, so a runaway `a..a..a..…`
        // chain (5.1 big.lua / 5.5 heavy.lua's `teststring`) raises the
        // overflow on the first pair that would exceed `INT_MAX` instead
        // of allocating multi-GB intermediates first.
        let max_str = i32::MAX as usize;
        if let (Value::Str(ls), Value::Str(rs)) = (l, r) {
            let a_len = ls.as_bytes().len();
            let b_len = rs.as_bytes().len();
            let new_len = a_len.checked_add(b_len);
            if new_len.is_none() || new_len.unwrap() > max_str {
                return Err(self.rt_err("string length overflow"));
            }
        }
        // a number operand is rendered on the stack; a string is used where
        // it is (both operands sit in registers, roots for the emergency
        // collection)
        let mut na = numeric::NumText::default();
        let mut nb = numeric::NumText::default();
        let (Some(a), Some(b)) = (
            concat_piece_into(l, legacy, &mut na),
            concat_piece_into(r, legacy, &mut nb),
        ) else {
            return Ok(None);
        };
        // PUC `MAX_SIZE` for Lua strings is `INT_MAX`; an attempt to
        // concat past it raises "string length overflow"
        // (5.5 heavy.lua `teststring` doubles `a..a..…` until it hits
        // exactly this wall).
        let new_len = a.len().checked_add(b.len());
        if new_len.is_none() || new_len.unwrap() > max_str {
            return Err(self.rt_err("string length overflow"));
        }
        let s = self.retry_oom(|vm| vm.heap.try_intern_pair(a, b))?;
        Ok(Some(Value::Str(s)))
    }

    /// Concatenate the operands in `[base_a .. self.top)` into one at
    /// `base_a` as PUC's `luaV_concat` does: from the top, each pass takes
    /// as many adjacent strings and numbers as there are and makes one
    /// string of them, a number first turned into a string in its own slot;
    /// a pair that needs `__concat` arms the metamethod call, whose `Meta`
    /// continuation re-enters here on its return.
    /// `out`: where the result goes once the operands from `base_a` up
    /// are folded into one (5.1–5.3 `CONCAT` names its own destination).
    pub(super) fn concat_run(&mut self, base_a: u32, out: u32) -> Result<(), LuaError> {
        let v = self.version;
        while self.top - base_a >= 2 {
            let top = self.top;
            let total = top - base_a;
            let x = self.stack[(top - 2) as usize];
            if !is_concat_piece(x) || !self.tostring_in_place(top - 1)? {
                let y = self.stack[(top - 1) as usize];
                let mut mm = self.get_mm(x, Mm::Concat);
                if mm.is_nil() {
                    mm = self.get_mm(y, Mm::Concat);
                }
                if mm.is_nil() {
                    let bad = if !is_concat_piece(x) { x } else { y };
                    return Err(self.type_err("concatenate", bad));
                }
                // the result lands at top-2, dropping y; resume continues
                let dst = top - 2;
                let out = (out as i64 - base_a as i64) as i16;
                self.begin_meta_call(mm, &[x, y], MetaAction::Concat { dst, base_a, out })?;
                return Ok(());
            }
            let Value::Str(y) = self.stack[(top - 1) as usize] else {
                unreachable!("converted above")
            };
            let mut n = 2;
            if y.is_empty() {
                // the result is the first operand, as a string
                self.tostring_in_place(top - 2)?;
            } else if v >= LuaVersion::Lua52 && matches!(x, Value::Str(s) if s.is_empty()) {
                self.stack[(top - 2) as usize] = Value::Str(y);
            } else {
                let mut tl = y.len();
                n = 1;
                while n < total && self.tostring_in_place(top - n - 1)? {
                    let Value::Str(s) = self.stack[(top - n - 1) as usize] else {
                        unreachable!("converted above")
                    };
                    if s.len() > i32::MAX as usize - tl {
                        return Err(self.rt_err("string length overflow"));
                    }
                    tl += s.len();
                    n += 1;
                }
                if v <= LuaVersion::Lua52 {
                    // made in the buffer kept for every concatenation
                    self.heap.open_gbuff(tl);
                }
                let r = self.retry_oom(|vm| {
                    let mut parts: [&[u8]; 16] = [&[]; 16];
                    let from = (top - n) as usize;
                    let strs = vm.stack[from..top as usize].iter().map(|v| match v {
                        // SAFETY: the operands sit in registers of the
                        // running instruction, which outlive the borrow
                        Value::Str(s) => unsafe { s.as_bytes_unbounded() },
                        _ => unreachable!("converted above"),
                    });
                    if n as usize <= parts.len() {
                        for (p, b) in parts.iter_mut().zip(strs) {
                            *p = b;
                        }
                        vm.heap.try_intern_parts(&parts[..n as usize])
                    } else {
                        let parts: Vec<&[u8]> = strs.collect();
                        vm.heap.try_intern_parts(&parts)
                    }
                })?;
                self.stack[(top - n) as usize] = Value::Str(r);
            }
            self.top -= n - 1;
        }
        if out != base_a {
            self.stack[out as usize] = self.stack[base_a as usize];
        }
        self.checkpoint(base_a + 1);
        Ok(())
    }

    /// PUC's `tostring` of a concatenation's operand in slot `slot`: a
    /// number becomes the string of it there (`luaO_tostring`, made as any
    /// string is); false for a value that is neither.
    fn tostring_in_place(&mut self, slot: u32) -> Result<bool, LuaError> {
        let v = self.stack[slot as usize];
        if matches!(v, Value::Str(_)) {
            return Ok(true);
        }
        let mut text = numeric::NumText::default();
        let Some(b) = concat_piece_into(v, self.float_fmt(), &mut text) else {
            return Ok(false);
        };
        let mut buf = [0u8; 64];
        let len = b.len();
        buf[..len].copy_from_slice(b);
        let s = self.retry_oom(|vm| vm.heap.try_intern(&buf[..len]))?;
        self.stack[slot as usize] = Value::Str(s);
        Ok(true)
    }

    /// `luaL_tolstring`: `__tostring` (whose result must be a string or a
    /// number, rendered), else the basic rendering, where 5.3+ names a value
    /// by a string `__name` metafield.
    pub fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        self.tostring_value_pushed(v, 0)
    }

    /// [`Vm::tostring_value`] from a native with `extra` values pushed
    /// where PUC's C function would have them: a `__tostring` it calls
    /// runs above them. Nothing is counted when there is none.
    pub(crate) fn tostring_value_pushed(
        &mut self,
        v: Value,
        extra: u32,
    ) -> Result<Vec<u8>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            // `luaL_callmeta` is a plain `lua_call`: `__tostring` cannot yield.
            self.native_push(extra);
            let r = self.call_value(mm, &[v])?;
            self.native_pop(extra);
            return match r.first().copied().unwrap_or(Value::Nil) {
                Value::Str(s) => Ok(s.as_bytes().to_vec()),
                r @ (Value::Int(_) | Value::Float(_)) => Ok(self.tostring_basic(r)),
                // luaL_error over the result: positioned at whatever called
                // the library function
                _ => {
                    self.native_push_if_native(1);
                    Err(crate::vm::builtins::raise_str(
                        self,
                        "'__tostring' must return a string",
                    ))
                }
            };
        }
        if self.version >= LuaVersion::Lua53
            && !matches!(
                v,
                Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Float(_) | Value::Str(_)
            )
            && let Value::Str(name) = self.get_mm(v, Mm::Name)
        {
            let basic = self.tostring_basic(v);
            let at = basic
                .iter()
                .position(|&c| c == b':')
                .expect("an object renders as `kind: address`");
            let mut out = name.as_bytes().to_vec();
            out.extend_from_slice(&basic[at..]);
            return Ok(out);
        }
        Ok(self.tostring_basic(v))
    }

    /// The dialect's float-rendering flavor: ≤5.2 %.14g
    /// bare, 5.3/5.4 %.14g + ".0", 5.5 two-stage %.15g/%.17g + ".0".
    pub(crate) fn float_fmt(&self) -> numeric::FloatFmt {
        use crate::version::LuaVersion::*;
        match self.version {
            Lua51 | Lua52 => numeric::FloatFmt::Legacy14,
            Lua53 | Lua54 => numeric::FloatFmt::G14,
            _ => numeric::FloatFmt::TwoStage55,
        }
    }

    /// Basic tostring (no metamethods).
    pub(crate) fn tostring_basic(&mut self, v: Value) -> Vec<u8> {
        match v {
            Value::Nil => b"nil".to_vec(),
            Value::Bool(true) => b"true".to_vec(),
            Value::Bool(false) => b"false".to_vec(),
            Value::Int(i) => numeric::num_to_string(Num::Int(i)).into_bytes(),
            // PUC ≤5.2 has no integer subtype — `tostring(2.0)` is `"2"`, not
            // `"2.0"`. The 5.3+ split needs the suffix so `print(2.0)` is
            // distinguishable from `print(2)`. pm.lua :13 builds patterns by
            // concatenating these renderings.
            Value::Float(f) => {
                numeric::num_to_string_for(Num::Float(f), self.float_fmt()).into_bytes()
            }
            Value::Str(s) => s.as_bytes().to_vec(),
            Value::Table(t) => format!("table: {}", c_pointer(t.as_ptr() as usize)).into_bytes(),
            Value::Closure(c) => {
                format!("function: {}", c_pointer(c.as_ptr() as usize)).into_bytes()
            }
            Value::Native(n) => {
                format!("function: {}", c_pointer(n.as_ptr() as usize)).into_bytes()
            }
            Value::Coro(co) => format!("thread: {}", c_pointer(co.as_ptr() as usize)).into_bytes(),
            // PUC names file handles `file (0x…)`; a bare userdata is
            // `userdata: 0x…`. The io library overrides this via __tostring.
            // a userdata the C API made is known to C by its block's address
            Value::Userdata(u) => format!(
                "userdata: {}",
                c_pointer(self.host_userdata_ptr(u) as usize)
            )
            .into_bytes(),
            // PUC `lua_topointer`/tostring on light udata: "userdata: 0x…"
            // (the "light" qualifier only appears in `luaL_typeerror`).
            Value::LightUserdata(p) => format!("userdata: {}", c_pointer(p as usize)).into_bytes(),
        }
    }
}
