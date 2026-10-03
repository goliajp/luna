//! Numeric `for` loop preparation and stepping.

use super::*;

impl Vm {
    // ---- numeric for ----

    /// Check and convert a numeric for's control values the way the
    /// dialect's `OP_FORPREP` does. The integer loop is chosen by the
    /// values' tags (a numeric string makes it a float loop, 5.3+); the
    /// check order, wording and the zero-step error differ per version:
    /// 5.1/5.2 test initial value, limit, step; 5.3+ limit, step, initial
    /// value; only 5.4+ reject a zero step, and an integer loop does that
    /// before looking at the limit.
    pub(super) fn for_operands(&mut self, base: u32, a: u32) -> Result<(Num, Num, Num), LuaError> {
        let (init, limit, step) = (self.r(base, a), self.r(base, a + 1), self.r(base, a + 2));
        let v = self.version();
        let order = if v <= LuaVersion::Lua52 {
            [("initial value", init), ("limit", limit), ("step", step)]
        } else {
            [("limit", limit), ("step", step), ("initial value", init)]
        };
        if v >= LuaVersion::Lua54 && matches!((init, step), (Value::Int(_), Value::Int(0))) {
            return Err(self.rt_err("'for' step is zero"));
        }
        for (what, val) in order {
            if as_num(val, v).is_none() {
                return Err(self.rt_err(&if v >= LuaVersion::Lua54 {
                    format!(
                        "bad 'for' {what} (number expected, got {})",
                        self.obj_typename(val)
                    )
                } else {
                    format!("'for' {what} must be a number")
                }));
            }
        }
        let n = |val| as_num(val, v).expect("checked above");
        let int_loop =
            v <= LuaVersion::Lua52 || matches!((init, step), (Value::Int(_), Value::Int(_)));
        if int_loop {
            return Ok((n(init), n(limit), n(step)));
        }
        let (i, l, st) = (n(init).as_f64(), n(limit).as_f64(), n(step).as_f64());
        if v >= LuaVersion::Lua54 && st == 0.0 {
            return Err(self.rt_err("'for' step is zero"));
        }
        Ok((Num::Float(i), Num::Float(l), Num::Float(st)))
    }

    pub(super) fn for_prep(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        let (init_n, limit_n, step_n) = self.for_operands(base, a)?;
        // PUC 5.1–5.3 `OP_FORPREP` stores `i = init - step` and *unconditionally*
        // jumps to the matching `OP_FORLOOP` — the body never runs ahead of the
        // first test, so each successful iteration emits a backward `OP_FORLOOP`
        // jump (db.lua's `for i=1,4 do a=1 end` ↦ 5 line-hook events instead of
        // 5.4's 4). 5.4+ collapsed that to a count-based fall-through. The skip
        // distance in luna's encoding is `loop_pc - prep_pc`; firing
        // `add_pc(bx - 1)` lands the running pc on OP_FORLOOP itself.
        let pre53 = self.version() <= LuaVersion::Lua53;
        // 5.1/5.2 have only doubles: PUC steps every loop in floating
        // point, so a loop over integers the VM keeps (`#t`) is a float
        // loop too. An integer loop there would wrap instead of rounding
        // and would compare with a floored limit (`for i = 1, 1.5, 0`
        // runs zero times on PUC, forever with the limit floored to 1).
        let dbl = self.version() <= LuaVersion::Lua52;
        match (init_n, step_n) {
            (Num::Int(i0), Num::Int(st)) if !dbl => {
                if pre53 {
                    // PUC 5.3 `forlimit`: int limit passes through; float limit
                    // gets clamped to MIN/MAX with a `stopnow` flag set only
                    // when the clamp is unreachable (positive float with a
                    // negative step → limit=MAX, stopnow; negative float with
                    // step>=0 → limit=MIN, stopnow). On `stopnow` PUC rewrites
                    // `init = 0` so OP_FORLOOP's first test against the
                    // unreachable clamp fails cleanly. An ordinary in-range
                    // empty loop (e.g. `for i = 1, 0`) is *not* `stopnow` — it
                    // lets OP_FORLOOP's natural test reject the first step.
                    let (lim, stopnow) = match limit_n {
                        Num::Int(l) => (l, false),
                        Num::Float(f) => {
                            // `luaV_tointeger` floors (ceils for a negative
                            // step); a float it cannot fit is clamped on
                            // the side of its sign, NaN counting as
                            // negative (`0 < n` is false).
                            let conv = if st < 0 { f.ceil() } else { f.floor() };
                            if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0)
                                .contains(&conv)
                            {
                                (conv as i64, false)
                            } else if f > 0.0 {
                                (i64::MAX, st < 0)
                            } else {
                                (i64::MIN, st >= 0)
                            }
                        }
                    };
                    let initv = if stopnow { 0 } else { i0 };
                    let pre = initv.wrapping_sub(st);
                    self.set_r(base, a, Value::Int(pre));
                    self.set_r(base, a + 1, Value::Int(lim));
                    self.set_r(base, a + 2, Value::Int(st));
                    self.add_pc(inst.bx() as i32 - 1);
                    return Ok(());
                }
                let (lim, empty) = int_for_limit(limit_n, i0, st);
                if empty {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                let count = if st > 0 {
                    (lim as u64).wrapping_sub(i0 as u64) / (st as u64)
                } else {
                    (i0 as u64).wrapping_sub(lim as u64) / (st as i128).unsigned_abs() as u64
                };
                self.set_r(base, a, Value::Int(i0));
                self.set_r(base, a + 1, Value::Int(count as i64));
                self.set_r(base, a + 2, Value::Int(st));
                self.set_r(base, a + 3, Value::Int(i0));
            }
            _ => {
                let (x0, lim, st) = (init_n.as_f64(), limit_n.as_f64(), step_n.as_f64());
                if pre53 {
                    let pre = x0 - st;
                    self.set_r(base, a, Value::Float(pre));
                    self.set_r(base, a + 1, Value::Float(lim));
                    self.set_r(base, a + 2, Value::Float(st));
                    self.add_pc(inst.bx() as i32 - 1);
                    return Ok(());
                }
                // lvm.c `forprep`: skip only when `0 < step ? limit < init :
                // init < limit`; a NaN makes both false, so the body runs
                // once (with a NaN step, on the second test's side)
                let skip = if 0.0 < st { lim < x0 } else { x0 < lim };
                let runs = !skip;
                if !runs {
                    self.add_pc(inst.bx() as i32);
                    return Ok(());
                }
                self.set_r(base, a, Value::Float(x0));
                self.set_r(base, a + 1, Value::Float(lim));
                self.set_r(base, a + 2, Value::Float(st));
                self.set_r(base, a + 3, Value::Float(x0));
            }
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn for_loop(&mut self, inst: Inst, base: u32) -> Result<(), LuaError> {
        let a = inst.a();
        // PUC 5.1–5.3 `OP_FORLOOP` compares the post-step `i` to `limit`
        // directly (R[a+1] holds the limit, *not* a remaining-count) so the
        // first iteration's test fires through the same backward-jump path as
        // every later iteration. 5.4+ switched to the count-based form luna
        // already uses for `Int`; the float branch was already PUC-3.x-style.
        let v = self.version();
        let pre53 = v <= LuaVersion::Lua53;
        // `for_prep` leaves the three slots all Int or all Float; anything
        // else was written by `debug.setlocal` or by crafted bytecode. PUC
        // reads such slots unchecked (garbage or a crash); luna raises.
        match (self.r(base, a), self.r(base, a + 1), self.r(base, a + 2)) {
            (Value::Int(cur), Value::Int(lim), Value::Int(st)) if pre53 => {
                let next = cur.wrapping_add(st);
                let cont = if st > 0 { next <= lim } else { next >= lim };
                if cont {
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            // the count is unsigned (PUC `lua_Unsigned`): a loop over
            // more than 2^63 values stores a "negative" one
            (Value::Int(cur), Value::Int(count), Value::Int(st)) => {
                if count != 0 {
                    let next = cur.wrapping_add(st);
                    self.set_r(base, a, Value::Int(next));
                    self.set_r(base, a + 1, Value::Int(count.wrapping_sub(1)));
                    self.set_r(base, a + 3, Value::Int(next));
                    self.add_pc(-(inst.bx() as i32));
                }
            }
            (Value::Float(cur), Value::Float(lim), Value::Float(st)) => {
                self.float_for_step(inst, base, cur, lim, st);
            }
            // 5.1/5.2 have one number type, so a number of the other
            // representation stored into a slot is still a valid state
            (x, l, s) if v <= LuaVersion::Lua52 => {
                match (as_number(x), as_number(l), as_number(s)) {
                    (Some(cur), Some(lim), Some(st)) => {
                        self.float_for_step(inst, base, cur.as_f64(), lim.as_f64(), st.as_f64())
                    }
                    _ => return Err(self.rt_err("'for' state corrupted")),
                }
            }
            _ => return Err(self.rt_err("'for' state corrupted")),
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn float_for_step(&mut self, inst: Inst, base: u32, cur: f64, lim: f64, st: f64) {
        let a = inst.a();
        let next = cur + st;
        let cont = if st > 0.0 { next <= lim } else { next >= lim };
        if cont {
            self.set_r(base, a, Value::Float(next));
            self.set_r(base, a + 3, Value::Float(next));
            self.add_pc(-(inst.bx() as i32));
        }
    }
}
