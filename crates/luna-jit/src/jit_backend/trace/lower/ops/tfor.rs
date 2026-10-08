use super::tfor_ipairs::emit_ipairs_tfor_call;
use super::*;

/// The generic-for iterator call.
pub(super) fn emit_tfor_call_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let OpHelpers { spill_id, .. } = lw.h.op;
    let OpCx { off, ins, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        // generic-for body tail. Sequence:
        //   1. Spill regs[A..=A+2] (iter / state / control) to
        //      vm.stack so the helper's
        //      `vm.stack[A+4..=A+6] = vm.stack[A..=A+2]` copy
        //      sees current trace values (control changes each
        //      iter via TForLoop's R[A+2] = R[A+4] writeback).
        //   2. Call `luna_jit_op_tforcall(A, nvars, ..)`. Status
        //      `< 0` → deopt (Lua-closure iter or runtime err).
        //   3. Continue branch: reload regs[A+2] + regs[A+4..]
        //      from vm.stack so subsequent body iters (after the
        //      back-edge from TForLoop) see iter results.
        //      current_kinds for reloaded slots = Unset; the
        //      first body iter still uses entry-tag kinds, and
        //      TForLoop tail's tag-check guards the back-edge
        //      so runtime types match emit-time assumptions.
        Op::TForCall | Op::TForCall53 | Op::TForCall55 => {
            let a_us = ins.a() as usize;
            let nvars = ins.c() as i64;
            let lay = ins.op().for_layout().expect("a loop op");
            let (ctl, first) = (a_us + lay.control() as usize, a_us + lay.var() as usize);
            // ipairs detection. Recorder's TForLoop trigger snapshots
            // `R[A]`'s library tag if Native; `ipairs`'s iterator
            // specialises emit into inline Table aget IR (skip the
            // `op_tforcall` C call entirely on the hot path).
            let is_ipairs_trace = record.tfor_iter == Some(luna_core::runtime::Builtin::IpairsIter);

            // spill discipline:
            // - non-ipairs case: spill R[A..=A+2] upfront (helper
            //   path runs unconditionally; needs vm.stack populated).
            // - ipairs case: SKIP the upfront spill on the hot
            //   path (R[A] and R[A+1] never change inside the
            //   trace — vm.stack still holds entry values, which
            //   is what the slow_blk helper reads). R[A+2] is
            //   spilled INSIDE slow_blk only, so fast iters pay
            //   nothing.
            let spill_ref = lw.bcx.import_func(spill_id);
            // a copy: the guard exits below take `lw` whole while this
            // closure lives, and nothing changes the kinds meanwhile
            let spill_kinds = lw.current_kinds.clone();
            let spill_slot = |bcx: &mut E, slot: usize| {
                let k = spill_kinds[off + slot];
                if k.untyped() {
                    return;
                }
                let raw_arg = bcx.use_var(regs[slot]);
                let tag_arg = emit_kind_tag(bcx, k, raw_arg).expect("typed");
                let slot_arg = bcx.ins().iconst(types::I64, slot as i64);
                bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
            };
            if !is_ipairs_trace {
                for slot in [a_us, a_us + 1, ctl] {
                    spill_slot(&mut lw.bcx, slot);
                }
            }

            if is_ipairs_trace {
                emit_ipairs_tfor_call(lw, pl, oc, a_us, nvars, &spill_slot);
            } else {
                emit_tfor_helper_call(lw, pl, oc, a_us, nvars);
            }

            lw.current_kinds[off + ctl] = RegKind::Unknown;
            lw.current_kinds[off + first] = RegKind::Unknown;
            if (nvars as usize) >= 2 && first + 1 < max_stack {
                lw.current_kinds[off + first + 1] = RegKind::Unknown;
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// The helper-call path (used by slow_blk in the
/// ipairs case + the non-ipairs case wholesale).
/// Allocates the 3-slot buffer, calls the helper,
/// brif-checks the result, def_vars regs + tag from
/// the buffer.
pub(super) fn emit_tfor_helper_call<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    a_us: usize,
    nvars: i64,
) {
    let Plan { max_stack, .. } = *pl;
    let Lower {
        tforcall_tag_var,
        tforcall_val_tag_var,
        ..
    } = *lw;
    let OpHelpers { op_tforcall_id, .. } = lw.h.op;
    let OpCx { i, rop, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    let out_ss = lw
        .bcx
        .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            24,
            3,
        ));
    let ctrl_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 0);
    let key_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 8);
    let val_addr = lw.bcx.ins().stack_addr(types::I64, out_ss, 16);
    // a native iterator can collect: R[A..=A+2] are on the stack (spilled
    // or never changed) and the registers below them go as roots
    let roots = emit_ssa_roots(lw, i, oc.off + a_us);
    let lay = oc.ins.op().for_layout().expect("a loop op");
    let (ctl, first) = (a_us + lay.control() as usize, a_us + lay.var() as usize);
    let a_arg = lw.bcx.ins().iconst(types::I64, a_us as i64);
    let nvars_arg = lw
        .bcx
        .ins()
        .iconst(types::I64, i64::from(lay.pack_call(nvars as u32)));
    let func_ref = lw.bcx.import_func(op_tforcall_id);
    let call_inst = lw.bcx.ins().call(
        func_ref,
        &[a_arg, nvars_arg, ctrl_addr, key_addr, val_addr, roots],
    );
    let status_or_tag = lw.bcx.inst_results(call_inst)[0];
    // -1: not a native iterator, or it raised; the
    // interpreter redoes the op
    let ok = lw
        .bcx
        .ins()
        .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, status_or_tag, 0);
    guard!(lw, pl, ok, i, rop.pc);
    // key tag | value tag << 8 (Vm::jit_op_tforcall)
    let key_tag = lw.bcx.ins().band_imm_u(status_or_tag, 0xff);
    let val_tag = lw.bcx.ins().ushr_imm_u(status_or_tag, 8);
    lw.bcx.def_var(tforcall_tag_var, key_tag);
    lw.bcx.def_var(tforcall_val_tag_var, val_tag);
    let ctrl_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 0);
    let key_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 8);
    let val_raw = lw.bcx.ins().stack_load(types::I64, types::I64, out_ss, 16);
    if lay.copies_control() {
        lw.bcx.def_var(regs[ctl], ctrl_raw);
    }
    lw.bcx.def_var(regs[first], key_raw);
    if (nvars as usize) >= 2 && first + 1 < max_stack {
        lw.bcx.def_var(regs[first + 1], val_raw);
    }
}
