//! The entry function the dispatcher calls, in front of the body.

use super::ChunkPlan;
use inkwell::context::Context;

/// LLVM's `fastcc` calling convention, for the body's calls to itself.
pub(super) const FASTCC: u32 = 8;

/// The entry the dispatcher calls: checks that the upvalue the body's
/// self-recursive calls go through still holds the running closure (they
/// are direct calls to the body, right only while it does; nothing the
/// body runs can reassign it), fills the self-call context
/// (`luna_jit_enter_ctx`: the native stack limit, the failure flag and
/// the calls the dialect allows), then runs the body.
pub(super) fn emit_entry<'ctx>(
    ctx: &'ctx Context,
    builder: &inkwell::builder::Builder<'ctx>,
    plan: &ChunkPlan,
    entry: inkwell::values::FunctionValue<'ctx>,
    body: inkwell::values::FunctionValue<'ctx>,
    helpers: &std::collections::HashMap<&'static str, inkwell::values::FunctionValue<'ctx>>,
) -> Option<()> {
    let i64_type = ctx.i64_type();
    let zero = i64_type.const_zero();
    let start = ctx.append_basic_block(entry, "entry");
    builder.position_at_end(start);
    let self_calls = plan
        .self_call_pcs
        .iter()
        .chain(&plan.tail_call_pcs)
        .any(|&c| c);
    if let Some(idx) = plan.self_upval_idx.filter(|_| self_calls) {
        let check = helpers.get("luna_jit_self_upval_check").copied()?;
        let idx_arg = i64_type.const_int(u64::from(idx), false);
        let ok = builder
            .build_call(check, &[idx_arg.into()], "self_check")
            .ok()?;
        let ok = match ok.try_as_basic_value() {
            inkwell::values::ValueKind::Basic(v) => v.into_int_value(),
            inkwell::values::ValueKind::Instruction(_) => return None,
        };
        let is_ok = builder
            .build_int_compare(inkwell::IntPredicate::NE, ok, zero, "self_ok")
            .ok()?;
        let go = ctx.append_basic_block(entry, "self_go");
        let deopt = ctx.append_basic_block(entry, "self_deopt");
        builder.build_conditional_branch(is_ok, go, deopt).ok()?;
        // the interpreter runs the call from its start, the level counted
        builder.position_at_end(deopt);
        let n = plan.num_params;
        let words = i64_type.array_type(2 * n.max(1));
        let buf = builder.build_alloca(words, "entry_args").ok()?;
        for (i, arg) in entry.get_param_iter().enumerate() {
            let at = |w: u64, name: &str| {
                // SAFETY: `buf` is an alloca of `2 * n` words and `w < 2 * n`
                unsafe {
                    builder.build_in_bounds_gep(
                        words,
                        buf,
                        &[i64_type.const_zero(), i64_type.const_int(w, false)],
                        name,
                    )
                }
                .ok()
            };
            let tag = u64::from(luna_core::__private::raw::INT);
            builder
                .build_store(at(2 * i as u64, "tag_at")?, i64_type.const_int(tag, false))
                .ok()?;
            builder
                .build_store(at(2 * i as u64 + 1, "arg_at")?, arg)
                .ok()?;
        }
        let record = helpers.get("luna_jit_handoff_frame").copied()?;
        let addr = builder
            .build_ptr_to_int(buf, i64_type, "entry_args_addr")
            .ok()?;
        let level0 = i64_type.const_int(u64::MAX, true);
        builder
            .build_call(
                record,
                &[
                    zero.into(),
                    level0.into(),
                    addr.into(),
                    i64_type.const_int(u64::from(n), false).into(),
                ],
                "handoff",
            )
            .ok()?;
        builder.build_return(Some(&zero)).ok()?;
        builder.position_at_end(go);
    }
    let (self_ctx, limit, base) = if self_calls {
        let words = i64_type.array_type(luna_jit_helpers::SELF_CTX_WORDS as u32);
        let at = builder.build_alloca(words, "self_ctx").ok()?;
        let addr = builder
            .build_ptr_to_int(at, i64_type, "self_ctx_addr")
            .ok()?;
        let fill = helpers.get("luna_jit_enter_ctx").copied()?;
        builder.build_call(fill, &[addr.into()], "fill_ctx").ok()?;
        let word = |w: u64, name: &str| -> Option<inkwell::values::IntValue<'ctx>> {
            let a = builder
                .build_int_add(addr, i64_type.const_int(8 * w, false), name)
                .ok()?;
            let p = builder
                .build_int_to_ptr(a, ctx.ptr_type(Default::default()), name)
                .ok()?;
            Some(builder.build_load(i64_type, p, name).ok()?.into_int_value())
        };
        (addr, word(0, "limit")?, word(3, "base")?)
    } else {
        (zero, zero, zero)
    };
    let mut args: Vec<inkwell::values::BasicMetadataValueEnum> =
        vec![self_ctx.into(), limit.into(), base.into(), zero.into()];
    args.extend(
        entry
            .get_param_iter()
            .map(Into::<inkwell::values::BasicMetadataValueEnum>::into),
    );
    let call = builder.build_call(body, &args, "body").ok()?;
    call.set_call_convention(FASTCC);
    let v = match call.try_as_basic_value() {
        inkwell::values::ValueKind::Basic(v) => v,
        inkwell::values::ValueKind::Instruction(_) => return None,
    };
    builder.build_return(Some(&v)).ok()?;
    Some(())
}
