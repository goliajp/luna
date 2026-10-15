//! Concatenation and `tostring` conversion.

use super::*;

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
}
