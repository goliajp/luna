//! The step sign of a 5.3 integer `for` loop, checked once before the
//! loop head. 5.3's `FORLOOP` compares the stepped index with the limit
//! one way for a positive step and the other way otherwise; the step
//! cannot change inside the loop, so the trace checks that its sign is the
//! one seen while recording and the loop tail compares one way only.

use super::*;

/// The step register of the loop that closes the trace and whether its
/// step was positive while recorded, when the trace can rely on that sign:
/// a 5.3 loop whose step is an integer the dispatcher checks on entry and
/// no recorded op writes.
pub(super) fn plan_step_guard(
    record: &TraceRecord,
    op_offsets: &[u32],
    head_live: &[bool],
    alt_paths: &[Option<alt_path::AltPath>],
    for_loop_idx: Option<usize>,
    pre53_int: bool,
) -> Option<(usize, bool)> {
    let i = for_loop_idx.filter(|_| pre53_int)?;
    let up = record.for_step_up?;
    let step = record.ops[i].inst.a() as usize + 2;
    let int_on_entry = head_live.get(step) == Some(&true)
        && record
            .entry_tags
            .get(step)
            .and_then(|&t| RegKind::from_entry_tag(t))
            == Some(RegKind::Int);
    let written = record.ops[..i]
        .iter()
        .zip(op_offsets)
        .any(|(rop, &off)| op_writes_at_offset(rop, off).any(|r| r as usize == step))
        || alt_path::alt_written(alt_paths).contains(&(step as u32));
    (int_on_entry && !written).then_some((step, up))
}

/// Fills the block before the loop head that checks the step's sign: a
/// different sign leaves at the head with the entry kinds before anything
/// has run.
pub(super) fn emit_step_precheck<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) {
    let Plan {
        record, max_stack, ..
    } = *pl;
    let (Some(block), Some((step, up))) = (lw.step_precheck, pl.step_guard) else {
        return;
    };
    let RuntimeHelpers {
        suppress_admit_id, ..
    } = lw.h.rt;
    let (reg_state, trace_fn_sig_ref, body_loop) =
        (lw.reg_state, lw.trace_fn_sig_ref, lw.body_loop);
    lw.bcx.switch_to_block(block);
    lw.bcx.seal_block(block);
    let entry_stored: Vec<Option<Value>> = lw
        .regs_full
        .iter()
        .map(|&v| Some(lw.bcx.use_var(v)))
        .collect();
    let s = lw.bcx.use_var(lw.regs_full[step]);
    let positive = lw.bcx.ins().icmp_imm_s(IntCC::SignedGreaterThan, s, 0);
    let exit_blk = lw.bcx.create_block();
    if up {
        lw.bcx.ins().brif(positive, body_loop, &[], exit_blk, &[]);
    } else {
        lw.bcx.ins().brif(positive, exit_blk, &[], body_loop, &[]);
    }
    lw.bcx.switch_to_block(exit_blk);
    lw.bcx.seal_block(exit_blk);
    let side_box: Box<TCellPtr> = Box::new(TCellPtr::null());
    let tags_idx = lw.per_exit_kinds.len() as u32;
    lw.per_exit_kinds.push((
        record.head_pc,
        lw.current_kinds[..max_stack].to_vec(),
        side_box,
    ));
    emit_tagged_exit(
        &mut lw.bcx,
        suppress_admit_id,
        &lw.regs_full[..max_stack],
        &entry_stored,
        reg_state,
        record.head_pc,
        record.head_pc,
        tags_idx,
        lw.flush_ctx.as_ref(),
        trace_fn_sig_ref,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use luna_core::version::LuaVersion;
    use luna_core::vm::isa::Inst;

    /// A closed record of `ops` (pcs from 0) with every register an
    /// integer on entry and the step sign `up`.
    fn record(ops: &[Inst], up: Option<bool>) -> TraceRecord {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let proto = vm
            .load(
                b"local a,b,c,d,e,f = 0,0,0,0,0,0; return a+b+c+d+e+f",
                b"=t",
            )
            .expect("compile")
            .proto;
        let tags = vec![luna_core::runtime::value::raw::INT; proto.max_stack as usize];
        let mut rec = TraceRecord::start(proto, 0, tags, false);
        for (pc, &inst) in ops.iter().enumerate() {
            assert!(rec.push(RecordedOp {
                proto,
                pc: pc as u32,
                inst,
                inline_depth: 0,
                var_count: None,
            }));
        }
        rec.closed = true;
        rec.for_step_up = up;
        rec
    }

    fn guard(rec: &TraceRecord, pre53_int: bool) -> Option<(usize, bool)> {
        let n = rec.ops.len();
        let live = vec![true; rec.head_proto.max_stack as usize];
        plan_step_guard(
            rec,
            &vec![0; n],
            &live,
            &vec![None; n],
            Some(n - 1),
            pre53_int,
        )
    }

    #[test]
    fn a_recorded_sign_is_checked_once() {
        let body = Inst::iabc(Op::Add, 5, 5, 4, false);
        let tail = Inst::iabx(Op::ForLoop, 0, 2);
        assert_eq!(
            guard(&record(&[body, tail], Some(true)), true),
            Some((2, true))
        );
        assert_eq!(
            guard(&record(&[body, tail], Some(false)), true),
            Some((2, false))
        );
    }

    #[test]
    fn no_check_outside_5_3_integer_loops_or_without_a_sign() {
        let tail = Inst::iabx(Op::ForLoop, 0, 1);
        assert_eq!(guard(&record(&[tail], Some(true)), false), None);
        assert_eq!(guard(&record(&[tail], None), true), None);
    }

    #[test]
    fn no_check_when_the_trace_writes_the_step() {
        // only `debug.setlocal` reaches the step register in real code
        let write = Inst::iabc(Op::Move, 2, 5, 0, false);
        let tail = Inst::iabx(Op::ForLoop, 0, 2);
        assert_eq!(guard(&record(&[write, tail], Some(true)), true), None);
    }
}
