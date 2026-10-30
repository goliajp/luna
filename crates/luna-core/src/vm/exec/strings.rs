//! Concatenation and `tostring` conversion.

use super::*;
use crate::runtime::puc::api::PucText;

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
    /// Whether it finished (`false`: the metamethod call is armed).
    pub(super) fn concat_run(&mut self, base_a: u32, out: u32) -> Result<bool, LuaError> {
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
                return Ok(false);
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
                n = self.concat_join(v, top, total, y.len())?;
            }
            self.top -= n - 1;
        }
        if out != base_a {
            self.stack[out as usize] = self.stack[base_a as usize];
        }
        Ok(true)
    }

    /// The longest run of strings (or numbers, made strings in place)
    /// below the top `top`, at most `total` of them, the topmost `y_len`
    /// bytes long, joined into one string in the run's lowest slot under
    /// dialect `v`: how many it joined.
    #[inline(always)]
    fn concat_join(
        &mut self,
        v: LuaVersion,
        top: u32,
        total: u32,
        y_len: usize,
    ) -> Result<u32, LuaError> {
        let mut tl = y_len;
        let mut n = 1;
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
            self.retry_oom(|vm| vm.heap.open_gbuff(tl))?;
        }
        let r = self.retry_oom(|vm| {
            let from = (top - n) as usize;
            if n == 2
                && let (Value::Str(a), Value::Str(b)) = (vm.stack[from], vm.stack[from + 1])
            {
                return vm.heap.try_intern_parts(&[a.as_bytes(), b.as_bytes()]);
            }
            let mut parts: [&[u8]; 16] = [&[]; 16];
            let strs = vm.stack[from..top as usize].iter().map(|v| match v {
                Value::Str(s) => s.as_bytes(),
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
        Ok(n)
    }

    /// The checkpoint after `CONCAT` finished in the running Lua function,
    /// its operands from `base_a` and its result at `out`, with each
    /// dialect's stack top: 5.1 the whole frame (taken before the result
    /// is copied, which changes nothing a collection sees); 5.2 and 5.3
    /// past the result or up to the operands, whichever is higher; later,
    /// past the result.
    pub(super) fn concat_checkpoint(&mut self, base_a: u32, out: u32) -> Result<(), LuaError> {
        let top = match self.version {
            LuaVersion::Lua51 => self.frame_top(),
            LuaVersion::Lua52 | LuaVersion::Lua53 => (out + 1).max(base_a),
            _ => out + 1,
        };
        // the result is in its register
        self.heap.rooted_safe_point();
        self.checkpoint(top)
    }

    /// PUC's `ci->top` of the running Lua function: its whole frame.
    pub(super) fn frame_top(&self) -> u32 {
        let f = self.top_frame();
        f.base + u32::from(f.closure.proto.max_stack)
    }

    /// PUC's `tostring` of a concatenation's operand in slot `slot`: a
    /// number becomes the string of it there (`luaO_tostring`, made as any
    /// string is); false for a value that is neither.
    #[inline(always)]
    fn tostring_in_place(&mut self, slot: u32) -> Result<bool, LuaError> {
        if matches!(self.stack[slot as usize], Value::Str(_)) {
            return Ok(true);
        }
        self.number_to_string_in_place(slot)
    }

    /// [`Vm::tostring_in_place`] of a value that is not a string.
    #[inline(never)]
    fn number_to_string_in_place(&mut self, slot: u32) -> Result<bool, LuaError> {
        let v = self.stack[slot as usize];
        let mut text = numeric::NumText::default();
        let Some(b) = concat_piece_into(v, self.float_fmt(), &mut text) else {
            return Ok(false);
        };
        let s = self.retry_oom(|vm| vm.heap.try_intern(b))?;
        self.stack[slot as usize] = Value::Str(s);
        Ok(true)
    }
}
