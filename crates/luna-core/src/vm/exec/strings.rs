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
        match (concat_piece(l, legacy), concat_piece(r, legacy)) {
            (Some(a), Some(b)) => {
                // PUC `MAX_SIZE` for Lua strings is `INT_MAX`; an attempt to
                // concat past it raises "string length overflow"
                // (5.5 heavy.lua `teststring` doubles `a..a..…` until it hits
                // exactly this wall).
                let new_len = a.len().checked_add(b.len());
                if new_len.is_none() || new_len.unwrap() > max_str {
                    return Err(self.rt_err("string length overflow"));
                }
                let mut combined = a;
                combined.extend_from_slice(&b);
                Ok(Some(Value::Str(self.heap.intern(&combined))))
            }
            _ => Ok(None),
        }
    }

    /// Fold the concat operands occupying `[base_a .. self.top)` right-to-left
    /// into a single result at `base_a` (PUC `luaV_concat`). Returns after
    /// either finishing (result at `base_a`) or arming a yieldable `__concat`
    /// call — its `Meta` continuation re-enters here on the metamethod's return.
    pub(super) fn concat_run(&mut self, base_a: u32) -> Result<(), LuaError> {
        // Sum the lengths of all all-Str operands BEFORE starting the
        // right-associative fold so a 129-operand `a..a..…` chain
        // (5.1 big.lua's `rep129(longs)`) raises overflow immediately,
        // not after dozens of multi-GB intermediate intern+hash rounds.
        // A non-Str operand falls through to the per-pair check.
        let max_str = i32::MAX as usize;
        let mut total: usize = 0;
        let mut all_str = true;
        for slot in base_a..self.top {
            match self.stack[slot as usize] {
                Value::Str(s) => match total.checked_add(s.as_bytes().len()) {
                    Some(t) if t <= max_str => total = t,
                    _ => return Err(self.rt_err("string length overflow")),
                },
                _ => {
                    all_str = false;
                    break;
                }
            }
        }
        let _ = all_str; // discrimination already captured by early returns above
        while self.top.saturating_sub(base_a) >= 2 {
            let i = self.top - 1; // rightmost operand
            let x = self.stack[(i - 1) as usize];
            let y = self.stack[i as usize];
            match self.concat_pair(x, y)? {
                Some(s) => {
                    self.stack[(i - 1) as usize] = s;
                    self.top = i; // consumed y
                }
                None => {
                    let mut mm = self.get_mm(x, Mm::Concat);
                    if mm.is_nil() {
                        mm = self.get_mm(y, Mm::Concat);
                    }
                    if mm.is_nil() {
                        let legacy = self.float_fmt();
                        let bad = if concat_piece(x, legacy).is_none() {
                            x
                        } else {
                            y
                        };
                        return Err(self.type_err("concatenate", bad));
                    }
                    // result lands at i-1, dropping y (top→i); resume continues.
                    let dst = i - 1;
                    self.begin_meta_call(mm, &[x, y], MetaAction::Concat { dst, base_a })?;
                    return Ok(());
                }
            }
        }
        self.maybe_collect_garbage(base_a + 1);
        Ok(())
    }

    /// `luaL_tolstring`: `__tostring` (whose result must be a string or a
    /// number, rendered), else the basic rendering, where 5.3+ names a value
    /// by a string `__name` metafield.
    pub fn tostring_value(&mut self, v: Value) -> Result<Vec<u8>, LuaError> {
        let mm = self.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            // `luaL_callmeta` is a plain `lua_call`: `__tostring` cannot yield.
            let r = self.call_noyield(mm, &[v])?;
            return match r.first().copied().unwrap_or(Value::Nil) {
                Value::Str(s) => Ok(s.as_bytes().to_vec()),
                r @ (Value::Int(_) | Value::Float(_)) => Ok(self.tostring_basic(r)),
                // luaL_error: positioned at whatever called the library function
                _ => Err(crate::vm::builtins::raise_str(
                    self,
                    "'__tostring' must return a string",
                )),
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
            Value::Userdata(u) => {
                format!("userdata: {}", c_pointer(u.as_ptr() as usize)).into_bytes()
            }
            // PUC `lua_topointer`/tostring on light udata: "userdata: 0x…"
            // (the "light" qualifier only appears in `luaL_typeerror`).
            Value::LightUserdata(p) => format!("userdata: {}", c_pointer(p as usize)).into_bytes(),
        }
    }
}
