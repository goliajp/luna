use super::*;

/// Closures, `close` and upvalue reads.
pub(super) fn emit_closure_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        head_proto,
        max_stack,
        effective_end,
        ..
    } = *pl;
    let OpHelpers {
        op_closure_id,
        spill_id,
        op_close_id,
        ..
    } = lw.h.op;
    let RuntimeHelpers { upval_get_id, .. } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        Op::Closure => {
            // R[A] := closure(proto.protos[Bx]).
            // Emit per-in_stack-upval spill followed by a
            // single op_closure helper call. Spill writes
            // vm.stack[base + d.index] = Value::pack(tag, raw)
            // so the helper's find_or_create_upval captures a
            // live slot. Restrictions enforced in pre-emit:
            // inline_depth == 0 + every in_stack source reg in
            // bounds. RegKind::Unset src → bail (no known tag).
            let bx = ins.bx() as usize;
            let inner = head_proto.protos[bx];
            let spill_ref = lw.module.declare_func_in_func(spill_id, lw.bcx.func);
            for d in inner.upvals.iter() {
                if !d.in_stack {
                    continue;
                }
                let src_idx = d.index as usize;
                let src_kind = lw.current_kinds[off + src_idx];
                // an untyped source cannot be packed to a Value
                let tag_byte = known_tag(src_kind)?;
                let slot_arg = lw.bcx.ins().iconst(types::I64, d.index as i64);
                let tag_arg = lw.bcx.ins().iconst(types::I64, tag_byte as i64);
                let raw_arg = lw.bcx.use_var(regs[src_idx]);
                lw.bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
            }
            let bx_arg = lw.bcx.ins().iconst(types::I64, ins.bx() as i64);
            let func_ref = lw.module.declare_func_in_func(op_closure_id, lw.bcx.func);
            let call = lw.bcx.ins().call(func_ref, &[bx_arg]);
            let v = lw.bcx.inst_results(call)[0];
            lw.bcx.def_var(regs[ins.a() as usize], v);
            lw.current_kinds[off + ins.a() as usize] = RegKind::Closure;
            lw.closure_seen += 1;
        }
        Op::Close => {
            // close open upvals at slot ≥ A.
            //
            // Sequence:
            //  1. Pre-Close spill every slot in [A..max_stack)
            //     whose current_kinds is known (helper's close_from
            //     reads vm.stack[s] to seal each upval, so the
            //     trace's IR Variable values must reach vm.stack
            //     first).
            //  2. Call `luna_jit_op_close(A)` → 0 (continue) or
            //     1 (deopt: handler would run / pre-pending_err).
            //  3. brif on the i64 status: continue_blk falls
            //     through to subsequent ops; deopt_blk does a
            //     full store_back of all regs and returns close_pc
            //     so the interpreter redoes the Op::Close cleanly.
            //
            // close_from is idempotent (open_upvals are popped on
            // first call), so a deopt that re-fires interp's
            // Op::Close → begin_close → close_from sees no work.
            let a_us = ins.a() as usize;
            let spill_ref = lw.module.declare_func_in_func(spill_id, lw.bcx.func);
            for slot in a_us..max_stack {
                let k = lw.current_kinds[off + slot];
                let Some(tag_byte) = known_tag(k) else {
                    continue;
                };
                let slot_arg = lw.bcx.ins().iconst(types::I64, slot as i64);
                let tag_arg = lw.bcx.ins().iconst(types::I64, tag_byte as i64);
                let raw_arg = lw.bcx.use_var(regs[slot]);
                lw.bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
            }
            let a_arg = lw.bcx.ins().iconst(types::I64, ins.a() as i64);
            let func_ref = lw.module.declare_func_in_func(op_close_id, lw.bcx.func);
            let call = lw.bcx.ins().call(func_ref, &[a_arg]);
            let status = lw.bcx.inst_results(call)[0];
            // 1: a `__close` handler would run; the interpreter
            // redoes the op and runs it
            let ok = lw.bcx.ins().icmp_imm_s(IntCC::Equal, status, 0);
            guard!(lw, pl, ok, i, rop.pc);
        }
        Op::GetUpval => {
            // R[A] := UpVal[B]. The helper reads JIT_CL's
            // upvals[B] and returns the raw 8-byte payload.
            // use-site inference (`infer_upval_exit`)
            // pins the kind when the immediate use is `Op::Call`
            // on R[A] (the call target must be a closure). Any
            // other shape leaves dispatchable=false. Per-side-exit
            // exit_tags guard side-exits firing
            // BEFORE this GetUpval: they snapshot the pre-GetUpval
            // current_kinds, so those exits restore as Untouched.
            //
            // memoize per upval idx via `upval_cache`.
            let idx_b = ins.b();
            let v = if let Some(&cached_var) = lw.upval_cache.get(&idx_b) {
                lw.bcx.use_var(cached_var)
            } else {
                let idx_arg = lw.bcx.ins().iconst(types::I64, ins.b() as i64);
                let func_ref = lw.module.declare_func_in_func(upval_get_id, lw.bcx.func);
                let call = lw.bcx.ins().call(func_ref, &[idx_arg]);
                let new_v = lw.bcx.inst_results(call)[0];
                let cache_var = lw.bcx.declare_var(types::I64);
                lw.bcx.def_var(cache_var, new_v);
                lw.upval_cache.insert(idx_b, cache_var);
                new_v
            };
            lw.bcx.def_var(regs[ins.a() as usize], v);
            // Look forward including the terminator (effective_end
            // is the Op::Call's index when truncation applies).
            let upper = effective_end.min(record.ops.len() - 1) + 1;
            let inferred = if i + 1 < upper {
                infer_upval_exit(ins.a(), &record.ops[i + 1..upper])
            } else {
                None
            };
            match inferred {
                Some(ExitTag::Closure) => {
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Closure;
                }
                _ => {
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Unknown;
                    lw.dispatchable = false;
                    lw.dispatch_off_reason =
                        lw.dispatch_off_reason.or(Some("GetUpval:not-Closure-use"));
                }
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}
