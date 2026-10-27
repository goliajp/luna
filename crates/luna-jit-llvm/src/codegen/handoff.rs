//! Handing a compiled level to the interpreter after a self call.

use super::emitter::ComputeEmitter;

impl<'ctx, 'a> ComputeEmitter<'ctx, 'a> {
    /// After the self call at `pc`: with the context's flag set, return at
    /// once (a call failed, or with `tail` the callee went to the
    /// interpreter as this level), or hand this level to the interpreter,
    /// going on after the call (the callee went there) or with it (the
    /// callee did not start).
    pub(super) fn after_self_call(&self, pc: usize, tail: bool) -> Option<()> {
        use luna_jit_helpers::{SELF_HANDED, SELF_NOT_STARTED};
        let builder = self.builder;
        let i64t = self.i64_type;
        let flag = self.ctx_word(1, "call_flag")?;
        let set = builder
            .build_int_compare(
                inkwell::IntPredicate::NE,
                flag,
                i64t.const_zero(),
                "flag_set",
            )
            .ok()?;
        let cold = self.ctx.append_basic_block(self.function, "flag_cold");
        let go = self.ctx.append_basic_block(self.function, "flag_go");
        builder.build_conditional_branch(set, cold, go).ok()?;
        builder.position_at_end(cold);
        let hand_at = |what: i64| {
            builder
                .build_int_compare(
                    inkwell::IntPredicate::EQ,
                    flag,
                    i64t.const_int(what as u64, false),
                    "flag_is",
                )
                .ok()
        };
        let not_started = hand_at(SELF_NOT_STARTED)?;
        let handed = hand_at(SELF_HANDED)?;
        let hand = if tail {
            not_started
        } else {
            builder.build_or(not_started, handed, "hand").ok()?
        };
        let hand_bb = self.ctx.append_basic_block(self.function, "hand");
        let ret_bb = self.ctx.append_basic_block(self.function, "flag_ret");
        builder
            .build_conditional_branch(hand, hand_bb, ret_bb)
            .ok()?;
        builder.position_at_end(ret_bb);
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        builder.position_at_end(hand_bb);
        let at = i64t.const_int(pc as u64, false);
        let after = i64t.const_int(pc as u64 + 1, false);
        let resume = builder
            .build_select(not_started, at, after, "resume")
            .ok()?;
        self.handoff(resume.into_int_value(), pc)?;
        builder.position_at_end(go);
        Some(())
    }

    /// Hand this level to the interpreter, which goes on with it at `pc`
    /// (a value), its registers as they are before the self call at
    /// `call_pc` (`luna_jit_handoff_frame`), and return.
    pub(super) fn handoff(
        &self,
        pc: inkwell::values::IntValue<'ctx>,
        call_pc: usize,
    ) -> Option<()> {
        // (an upvalue check failing parked a deopt the record replaces)
        use luna_core::__private::raw;
        let builder = self.builder;
        let i64t = self.i64_type;
        let n = self.num_regs;
        let words = i64t.array_type(2 * n);
        let buf = builder.build_alloca(words, "handoff_regs").ok()?;
        let nils = self.nils_at.get(call_pc).cloned().flatten()?;
        for r in 0..n {
            let nil = nils.get(r as usize).copied().unwrap_or(true);
            let tag = if nil { raw::NIL } else { raw::INT };
            let at = |w: u32, name: &str| {
                // SAFETY: `buf` is an alloca of `2 * n` words and `w < 2 * n`
                unsafe {
                    builder.build_in_bounds_gep(
                        words,
                        buf,
                        &[i64t.const_zero(), i64t.const_int(u64::from(w), false)],
                        name,
                    )
                }
                .ok()
            };
            builder
                .build_store(at(2 * r, "tag_at")?, i64t.const_int(u64::from(tag), false))
                .ok()?;
            if !nil {
                let v = self.load_reg(r, "handoff_reg")?;
                builder.build_store(at(2 * r + 1, "val_at")?, v).ok()?;
            }
        }
        let addr = builder.build_ptr_to_int(buf, i64t, "handoff_addr").ok()?;
        self.record_level(pc, addr)?;
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        Some(())
    }

    /// `luna_jit_handoff_frame` of this level, its registers at `regs`: the
    /// level below is the interpreter's (-1) unless the chunk calls itself,
    /// when the self-call context knows its base, and a level of such a
    /// chunk sets the context's flag for the one below.
    fn record_level(
        &self,
        pc: inkwell::values::IntValue<'ctx>,
        regs: inkwell::values::IntValue<'ctx>,
    ) -> Option<()> {
        let i64t = self.i64_type;
        let base = if self.self_calls {
            self.function.get_nth_param(2)?.into_int_value()
        } else {
            i64t.const_all_ones()
        };
        let record = self.helpers.get("luna_jit_handoff_frame").copied()?;
        let n = i64t.const_int(u64::from(self.num_regs), false);
        let args = [pc.into(), base.into(), regs.into(), n.into()];
        self.builder.build_call(record, &args, "handoff").ok()?;
        if self.self_calls {
            let flag_at = self.ctx_addr(1, "flag_at")?;
            let handed = i64t.const_int(luna_jit_helpers::SELF_HANDED as u64, false);
            self.builder.build_store(flag_at, handed).ok()?;
        }
        Some(())
    }
}
