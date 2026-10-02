use super::*;

/// Inlined calls and returns.
pub(super) fn emit_call_op<M: Module>(
    lw: &mut Lower<'_, '_, M>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        effective_end,
        self_link_idx_opt,
        ..
    } = *pl;
    let RuntimeHelpers {
        head_closure_id, ..
    } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = &oc.regs;
    match oc.op {
        // inline self-recursive Call: emit nothing.
        // The recorder's depth bump (next op at depth+1) drives the
        // op_offsets shift; subsequent emit lands in the callee's
        // register window via the `off` shadow.
        //
        // push the callee frame onto `call_chain`
        // so subsequent cmp@d>0 sites can snapshot the chain. The
        // pushed `pc` is the caller's resume PC (Call.pc + 1); the
        // innermost frame's pc is overwritten with the side-exit PC
        // at snapshot time.
        Op::Call => {
            // The inlined body is the head proto's code run with the
            // entry closure's upvalues, which is right only when the
            // callee is that very closure. Anything else (another
            // closure of the proto, a reassigned upvalue, another
            // function) leaves here and the interpreter makes the call.
            let callee_reg = ins.a() as usize;
            if !matches!(lw.current_kinds[off + callee_reg], RegKind::Closure) {
                checkpoint("bail:inline-callee-not-closure");
                return None;
            }
            let head_cl = match lw.head_closure_var {
                Some(var) => lw.bcx.use_var(var),
                None => {
                    let func_ref = lw.module.declare_func_in_func(head_closure_id, lw.bcx.func);
                    let call = lw.bcx.ins().call(func_ref, &[]);
                    let v = lw.bcx.inst_results(call)[0];
                    let var = lw.bcx.declare_var(types::I64);
                    lw.bcx.def_var(var, v);
                    lw.head_closure_var = Some(var);
                    v
                }
            };
            let callee = lw.bcx.use_var(regs[callee_reg]);
            let same = lw.bcx.ins().icmp(IntCC::Equal, callee, head_cl);
            guard!(lw, pl, same, i, rop.pc);
            // SelfLink close: the LAST recorded op is the
            // Op::Call whose "next" op (the tripping deepest-depth
            // entry) was never captured. Skip the call_chain push
            // for that trailing Call — the SelfLink tail emit
            // computes its bump_off from this Call's offset + A + 1
            // directly. No FrameMaterializeInfo needed because no
            // side-exit can fire inside the tripping callee (it has
            // no recorded body).
            if self_link_idx_opt.is_some() && i + 1 == effective_end {
                return Some(());
            }
            // Next op is at depth+1 (recorder invariant for
            // self-recursive entry); its op_offsets entry is the
            // callee's base_offset.
            debug_assert!(
                i + 1 < effective_end,
                "self-rec Call must be followed by callee op in effective_end"
            );
            let callee_base = pl.op_offsets[i + 1];
            lw.call_chain.push(FrameMaterializeInfo {
                base_offset: callee_base,
                pc: rop.pc + 1,
                nresults: 1,
            });
        }
        // inline Return0: callee returns no values
        // back to the caller. The caller's R[call_a..] slots stay
        // whatever the caller had written (Lua semantics: the
        // return values are nil if the caller's call expected
        // more than the callee delivered; here recorder snapshots
        // a single concrete trip so trust the recorded trace).
        // pop the matching call_chain frame.
        Op::Return0 => {
            debug_assert!(
                !lw.call_chain.is_empty(),
                "Return0 at depth>0 has a matching frame"
            );
            lw.call_chain.pop();
        }
        // inline Return1: copy callee's R[A]
        // into the caller's R[call_a]. `op_offsets` for the
        // following ops will revert to the caller's window, but
        // the value lives in `regs_full[caller_off + call_a]`
        // ready for the caller's continuation to read it.
        Op::Return1 => {
            let a_callee = ins.a() as usize;
            let call_a = pl.enclosing_call_a[i]
                .expect("Return1 at depth>0 has an enclosing Op::Call")
                as usize;
            // Caller window's offset is below ours by call_a+1
            // (callee R[0] sits at caller R[call_a+1]).
            let caller_off = off
                .checked_sub(call_a + 1)
                .expect("op_offsets invariant: callee window > caller window");
            let src_var = lw.regs_full[off + a_callee];
            let dst_var = lw.regs_full[caller_off + call_a];
            let v = lw.bcx.use_var(src_var);
            lw.bcx.def_var(dst_var, v);
            // Propagate the kind so the caller's continuation
            // sees the right type.
            lw.current_kinds[caller_off + call_a] = lw.current_kinds[off + a_callee];
            // pop matching call_chain frame.
            debug_assert!(
                !lw.call_chain.is_empty(),
                "Return1 at depth>0 has a matching frame"
            );
            lw.call_chain.pop();
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}
