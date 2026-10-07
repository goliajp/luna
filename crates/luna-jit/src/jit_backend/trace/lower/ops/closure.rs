use super::*;

/// Closures, `close` and upvalue reads.
pub(super) fn emit_closure_op<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan {
        record,
        effective_end,
        ..
    } = *pl;
    let OpHelpers {
        op_closure_id,
        spill_id,
        ..
    } = lw.h.op;
    let RuntimeHelpers { upval_get_id, .. } = lw.h.rt;
    let OpCx {
        i, rop, off, ins, ..
    } = *oc;
    let regs: &[Variable] = oc.regs;
    match oc.op {
        Op::Closure => {
            // R[A] := closure(proto.protos[Bx]). The in-stack upvalues'
            // sources go to the stack first, where the helper opens the
            // upvalues on the frame's slots. In the head frame the helper
            // takes the head closure; in an inlined frame, that frame's own
            // closure and where its registers start.
            let bx = ins.bx() as usize;
            let inner = rop.proto.protos[bx];
            let spill_ref = lw.bcx.import_func(spill_id);
            for d in inner.upvals.iter() {
                if !d.in_stack {
                    continue;
                }
                let src_idx = d.index as usize;
                let src_kind = lw.current_kinds[off + src_idx];
                let raw_arg = lw.bcx.use_var(regs[src_idx]);
                // an untyped source cannot be packed to a Value
                let tag_arg = emit_kind_tag(&mut lw.bcx, src_kind, raw_arg)?;
                let slot_arg = lw.bcx.ins().iconst(types::I64, (off + src_idx) as i64);
                lw.bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
            }
            let bx_arg = lw.bcx.ins().iconst(types::I64, ins.bx() as i64);
            let call = if rop.inline_depth == 0 {
                let func_ref = lw.bcx.import_func(op_closure_id);
                lw.bcx.ins().call(func_ref, &[bx_arg])
            } else {
                let cl = lw.bcx.use_var(lw.regs_full[pl.frame_func[i] as usize]);
                let off_arg = lw.bcx.ins().iconst(types::I64, off as i64);
                let func_ref = lw.bcx.import_func(lw.h.op.op_closure_in_id);
                lw.bcx.ins().call(func_ref, &[cl, bx_arg, off_arg])
            };
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
            close_from(lw, pl, oc, ins.a() as usize)?;
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
            if !std::ptr::eq(rop.proto.as_ptr(), pl.head_proto.as_ptr()) {
                return emit_frame_upval_op(lw, pl, oc);
            }
            let v = if let Some(&cached_var) = lw.upval_cache.get(&idx_b) {
                lw.bcx.use_var(cached_var)
            } else {
                let idx_arg = lw.bcx.ins().iconst(types::I64, ins.b() as i64);
                let func_ref = lw.bcx.import_func(upval_get_id);
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
            // otherwise typed by the value the recording saw, the read
            // checked against it once, where it is made
            let seen = getx_want(record.result_tag(i).and_then(|t| {
                use luna_core::runtime::value::raw;
                match t {
                    raw::INT => Some(ExitTag::Int),
                    raw::FLOAT => Some(ExitTag::Float),
                    raw::TABLE => Some(ExitTag::Table),
                    raw::STR => Some(ExitTag::Str),
                    _ => None,
                }
            }));
            // a call target is not always a Lua closure (a table with
            // `__call`, a native): the register is typed Closure only when
            // the recording saw one, and the read is checked, so an exit
            // never restores some other object under the closure tag
            let seen_closure =
                record.result_tag(i) == Some(luna_core::runtime::value::raw::CLOSURE);
            match inferred {
                Some(ExitTag::Closure) if seen_closure => {
                    let want = luna_core::runtime::value::raw::CLOSURE;
                    let v = checked_upval_read(lw, pl, oc.i, oc.rop, idx_b, want)?;
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] = RegKind::Closure;
                }
                _ if let Some((kind, want)) = seen => {
                    let v = checked_upval_read(lw, pl, oc.i, oc.rop, idx_b, want)?;
                    lw.bcx.def_var(regs[ins.a() as usize], v);
                    lw.current_kinds[off + ins.a() as usize] = kind;
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

/// Upvalue `idx` read through the checked helper, typed `want`: the call
/// made once per trace, at the first read, and guarded there.
fn checked_upval_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
    want: u8,
) -> Option<Value> {
    let RuntimeHelpers {
        upval_get_checked_id,
        ..
    } = lw.h.rt;
    let bcx = &mut lw.bcx;
    let checked = lw.upval_checked.entry(idx).or_insert_with(|| {
        let var = bcx.declare_var(types::I64);
        let ss = bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        let out = bcx.ins().stack_addr(types::I64, ss, 0);
        let idx_arg = bcx.ins().iconst(types::I64, i64::from(idx));
        let want_arg = bcx.ins().iconst(types::I64, i64::from(want));
        let f = bcx.import_func(upval_get_checked_id);
        let call = bcx.ins().call(f, &[idx_arg, want_arg, out]);
        let ok = bcx.inst_results(call)[0];
        (var, ss, ok, want)
    });
    let (var, ss, ok, checked_want) = *checked;
    if checked_want != want {
        return None;
    }
    // the first read of this upvalue made the check
    if !lw.upval_check_done.contains(&idx) {
        lw.upval_check_done.push(idx);
        guard!(lw, pl, ok, i, rop.pc);
        let v = lw.bcx.ins().stack_load(types::I64, types::I64, ss, 0);
        lw.bcx.def_var(var, v);
    }
    Some(lw.bcx.use_var(var))
}

/// The table in upvalue `idx` of op `i`'s function, checked to be a table
/// (the trace leaves at the op otherwise).
pub(in crate::jit_backend::trace::lower) fn upval_table_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
) -> Option<Value> {
    let table = luna_core::runtime::value::raw::TABLE;
    if std::ptr::eq(rop.proto.as_ptr(), pl.head_proto.as_ptr()) {
        checked_upval_read(lw, pl, i, rop, idx, table)
    } else {
        Some(frame_upval_read(lw, pl, i, rop, idx, table))
    }
}

/// `GetUpval` in a function of another proto the trace inlined: read
/// through that frame's own closure, typed by the value the recording saw.
fn emit_frame_upval_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    let OpCx { i, off, ins, .. } = *oc;
    let Some(kind) = pl
        .record
        .result_tag(i)
        .and_then(RegKind::from_entry_tag)
        .filter(|k| !matches!(k, RegKind::Nil | RegKind::Bool))
    else {
        checkpoint("bail:inline-upval-untyped");
        return None;
    };
    let v = frame_upval_read(lw, pl, i, oc.rop, ins.b(), kind_tag(kind));
    lw.bcx.def_var(oc.regs[ins.a() as usize], v);
    lw.current_kinds[off + ins.a() as usize] = kind;
    Some(())
}

/// Upvalue `idx` of the closure running the inlined frame of op `oc` (the
/// value its caller called, one below the frame's window), checked to
/// have raw tag `want`; the trace leaves at the op otherwise.
pub(super) fn frame_upval_read<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    rop: &RecordedOp,
    idx: u32,
    want: u8,
) -> Value {
    let RuntimeHelpers {
        upval_of_checked_id,
        ..
    } = lw.h.rt;
    let cl = lw.bcx.use_var(lw.regs_full[pl.frame_func[i] as usize]);
    let idx_arg = lw.bcx.ins().iconst(types::I64, i64::from(idx));
    let ss = lw
        .bcx
        .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
    let out = lw.bcx.ins().stack_addr(types::I64, ss, 0);
    let want_arg = lw.bcx.ins().iconst(types::I64, i64::from(want));
    let f = lw.bcx.import_func(upval_of_checked_id);
    let call = lw.bcx.ins().call(f, &[cl, idx_arg, want_arg, out]);
    let ok = lw.bcx.inst_results(call)[0];
    guard!(lw, pl, ok, i, rop.pc);
    lw.bcx.ins().stack_load(types::I64, types::I64, ss, 0)
}

/// `close` from register `a` of the frame of op `oc` on: its registers
/// the trace holds a kind for go to the stack first (the helper seals each
/// upvalue with the stack's value), then the helper closes the upvalues at
/// those slots. A `__close` handler to run leaves the trace at the op, for
/// the interpreter to run it.
fn close_from<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>, a: usize) -> Option<()> {
    let OpCx { i, rop, off, .. } = *oc;
    let spill_ref = lw.bcx.import_func(lw.h.op.spill_id);
    for slot in a..rop.proto.max_stack as usize {
        let k = lw.current_kinds[off + slot];
        if k.untyped() {
            continue;
        }
        let raw_arg = lw.bcx.use_var(lw.regs_full[off + slot]);
        let tag_arg = emit_kind_tag(&mut lw.bcx, k, raw_arg).expect("typed");
        let slot_arg = lw.bcx.ins().iconst(types::I64, (off + slot) as i64);
        lw.bcx.ins().call(spill_ref, &[slot_arg, tag_arg, raw_arg]);
    }
    let a_arg = lw.bcx.ins().iconst(types::I64, (off + a) as i64);
    let func_ref = lw.bcx.import_func(lw.h.op.op_close_id);
    let call = lw.bcx.ins().call(func_ref, &[a_arg]);
    let status = lw.bcx.inst_results(call)[0];
    // 1: a `__close` handler would run; the interpreter redoes the op and
    // runs it
    let ok = lw.bcx.ins().icmp_imm_s(IntCC::Equal, status, 0);
    guard!(lw, pl, ok, i, rop.pc);
    Some(())
}

/// The return of an inlined function that closes its frame's upvalues
/// (a closure it made captured one of its locals).
pub(super) fn close_frame<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    close_from(lw, pl, oc, 0)
}
