//! A self-recursive call compiled as a direct call to the body itself:
//! the guard at the body's start and the levels counted for PUC.

use super::emitter::ComputeEmitter;
use luna_core::vm::isa::Inst;

impl<'ctx, 'a> ComputeEmitter<'ctx, 'a> {
    /// Self-recursive `Op::Call`: a direct call to the entry function,
    /// result into regs[A], then return early if a deopt was parked.
    pub(super) fn emit_self_call(&self, ins: Inst, pc: usize) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "call_arg")?;
            arg_vals.push(v.into());
        }
        let v = self.guarded_self_call(&arg_vals, a, None, "self_call")?;
        // a call below that failed returns at once; one that went to the
        // interpreter takes this level with it
        self.after_self_call(pc, false)?;
        let slot = self.reg_slot_ptr(a, "call_dst")?;
        builder.build_store(slot, v).ok()?;
        Some(())
    }

    /// Self-recursive `Op::TailCall`: call the entry function and return
    /// its result (`Op::Call` + `Op::Return1` fused).
    pub(super) fn emit_self_tail_call(&self, ins: Inst, pc: usize) -> Option<()> {
        let builder = self.builder;
        let a = ins.a();
        let nargs = ins.b().saturating_sub(1);
        let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> =
            Vec::with_capacity(nargs as usize);
        for off in 1..=nargs {
            let v = self.load_reg(a + off, "tail_arg")?;
            arg_vals.push(v.into());
        }
        let v = self.guarded_self_call(&arg_vals, a, Some(pc), "tail_call")?;
        // the callee took this level: one that went to the interpreter
        // took it there already
        self.after_self_call(pc, true)?;
        // Return the tail call result directly — no regs[A] store.
        builder.build_return(Some(&v)).ok()?;
        Some(())
    }

    /// The address of word `w` of the self-call context (the body's first
    /// parameter).
    pub(super) fn ctx_addr(
        &self,
        w: u64,
        name: &str,
    ) -> Option<inkwell::values::PointerValue<'ctx>> {
        let base = self.function.get_nth_param(0)?.into_int_value();
        let off = self.i64_type.const_int(8 * w, false);
        let at = self.builder.build_int_add(base, off, name).ok()?;
        self.builder
            .build_int_to_ptr(at, self.ctx.ptr_type(Default::default()), name)
            .ok()
    }

    pub(super) fn ctx_word(&self, w: u64, name: &str) -> Option<inkwell::values::IntValue<'ctx>> {
        let p = self.ctx_addr(w, name)?;
        Some(
            self.builder
                .build_load(self.i64_type, p, name)
                .ok()?
                .into_int_value(),
        )
    }

    /// Return 0 from the chunk unless `ok`, else go on in a fresh block.
    pub(super) fn return_if_not(
        &self,
        ok: inkwell::values::IntValue<'ctx>,
        name: &str,
    ) -> Option<()> {
        let go = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_go"));
        let back = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_back"));
        self.builder.build_conditional_branch(ok, go, back).ok()?;
        self.builder.position_at_end(back);
        self.builder
            .build_return(Some(&self.i64_type.const_zero()))
            .ok()?;
        self.builder.position_at_end(go);
        Some(())
    }

    /// A self call: the body itself, at the level above this one's
    /// register `a`. The body checks on entry whether it may run natively
    /// and counts its level when it must (`emit_self_guard`), so the call
    /// is a plain call LLVM can turn into a loop. A tail call reuses this
    /// level; PUC places the callee above it first, which is counted here
    /// (`luna_jit_self_level` with [`SELF_LEVEL_TAIL`]).
    pub(super) fn guarded_self_call(
        &self,
        args: &[inkwell::values::BasicMetadataValueEnum<'ctx>],
        a: u32,
        tail: Option<usize>,
        name: &str,
    ) -> Option<inkwell::values::IntValue<'ctx>> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let base = self.function.get_nth_param(2)?.into_int_value();
        let depth = self.function.get_nth_param(3)?.into_int_value();
        let up = builder
            .build_int_add(base, i64t.const_int(u64::from(a) + 1, false), "up_base")
            .ok()?;
        let deeper = builder
            .build_int_add(depth, i64t.const_int(1, false), "up_depth")
            .ok()?;
        let (base, depth) = if let Some(pc) = tail {
            let desc = self.self_call_desc | luna_jit_helpers::SELF_LEVEL_TAIL;
            self.count_level(up, deeper, desc, Some(pc), name)?;
            (base, depth)
        } else {
            (up, deeper)
        };
        let mut body_args: Vec<inkwell::values::BasicMetadataValueEnum<'ctx>> = vec![
            self.function.get_nth_param(0)?.into(),
            self.function.get_nth_param(1)?.into(),
            base.into(),
            depth.into(),
        ];
        body_args.extend_from_slice(args);
        let call = builder.build_call(self.function, &body_args, name).ok()?;
        call.set_call_convention(self.function.get_call_conventions());
        match call.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(bv) => Some(bv.into_int_value()),
            inkwell::values::ValueKind::Instruction(_) => None,
        }
    }

    /// The level at `base` and `depth` counted when it reaches the limits
    /// in the context (`luna_jit_self_level`); return 0 when that fails.
    pub(super) fn count_level(
        &self,
        base: inkwell::values::IntValue<'ctx>,
        depth: inkwell::values::IntValue<'ctx>,
        desc: i64,
        tail: Option<usize>,
        name: &str,
    ) -> Option<()> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let base_limit = self.ctx_word(2, "base_limit")?;
        let cross = builder
            .build_int_compare(inkwell::IntPredicate::SGE, base, base_limit, "high")
            .ok()?;
        let count_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_level"));
        let go_bb = self
            .ctx
            .append_basic_block(self.function, &format!("{name}_level_go"));
        builder
            .build_conditional_branch(cross, count_bb, go_bb)
            .ok()?;
        builder.position_at_end(count_bb);
        let level_fn = self.helpers.get("luna_jit_self_level").copied()?;
        let ctx_arg = self.function.get_nth_param(0)?.into_int_value();
        let r = builder
            .build_call(
                level_fn,
                &[
                    ctx_arg.into(),
                    i64t.const_int(desc as u64, false).into(),
                    base.into(),
                    depth.into(),
                ],
                "level",
            )
            .ok()?;
        let r = match r.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let ok = builder
            .build_int_compare(inkwell::IntPredicate::EQ, r, i64t.const_zero(), "level_ok")
            .ok()?;
        match tail {
            // the tail call did not start: this level goes with it
            Some(pc) => {
                let failed_bb = self
                    .ctx
                    .append_basic_block(self.function, &format!("{name}_level_failed"));
                builder
                    .build_conditional_branch(ok, go_bb, failed_bb)
                    .ok()?;
                builder.position_at_end(failed_bb);
                self.after_self_call(pc, true)?;
                builder.build_return(Some(&i64t.const_zero())).ok()?;
            }
            None => {
                self.return_if_not(ok, &format!("{name}_level"))?;
                builder.build_unconditional_branch(go_bb).ok()?;
            }
        }
        builder.position_at_end(go_bb);
        Some(())
    }

    /// At the body's start, when it was entered by a self call: with the
    /// native stack below the context's limit, leave this call to the
    /// interpreter (the caller hands itself over), as the Cranelift tier's
    /// stub does; otherwise count the level when it must be.
    pub(super) fn emit_self_guard(&self) -> Option<()> {
        let builder = self.builder;
        let i64t = self.i64_type;
        let limit = self.function.get_nth_param(1)?.into_int_value();
        let base = self.function.get_nth_param(2)?.into_int_value();
        let depth = self.function.get_nth_param(3)?.into_int_value();
        // x86_64 or aarch64, the architectures `lib.rs` admits
        let sp_name = if cfg!(target_arch = "x86_64") {
            "rsp"
        } else {
            "sp"
        };
        let sp_reg = self
            .ctx
            .metadata_node(&[self.ctx.metadata_string(sp_name).into()]);
        let sp = builder
            .build_call(self.read_register, &[sp_reg.into()], "sp")
            .ok()?;
        let sp = match sp.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let slow = builder
            .build_int_compare(inkwell::IntPredicate::ULT, sp, limit, "stack_low")
            .ok()?;
        let slow_bb = self.ctx.append_basic_block(self.function, "self_slow");
        let go_bb = self.ctx.append_basic_block(self.function, "self_native");
        builder
            .build_conditional_branch(slow, slow_bb, go_bb)
            .ok()?;
        builder.position_at_end(slow_bb);
        // the interpreter makes the call
        let flag_at = self.ctx_addr(1, "flag_at")?;
        let not_started = i64t.const_int(luna_jit_helpers::SELF_NOT_STARTED as u64, false);
        builder.build_store(flag_at, not_started).ok()?;
        builder.build_return(Some(&i64t.const_zero())).ok()?;
        builder.position_at_end(go_bb);
        self.count_level(base, depth, self.self_call_desc, None, "entry")
    }
}
