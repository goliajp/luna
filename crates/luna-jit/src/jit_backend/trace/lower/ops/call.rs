use super::*;

/// Inlined calls and returns.
pub(super) fn emit_call_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
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
    let regs: &[Variable] = oc.regs;
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
            let callee_reg = ins.a() as usize;
            if !matches!(lw.current_kinds[off + callee_reg], RegKind::Closure) {
                checkpoint("bail:inline-callee-not-closure");
                return None;
            }
            let callee = lw.bcx.use_var(regs[callee_reg]);
            // the function the recording went into: the next op's (a
            // SelfLink close ends at the call into the head function)
            let callee_proto = pl.record.ops.get(i + 1).map_or(pl.head_proto, |r| r.proto);
            if std::ptr::eq(callee_proto.as_ptr(), pl.head_proto.as_ptr()) {
                // The inlined body is the head proto's code run with the
                // entry closure's upvalues, which is right only when the
                // callee is that very closure. Anything else (another
                // closure of the proto, a reassigned upvalue, another
                // function) leaves here and the interpreter makes the call.
                let head_cl = match lw.head_closure_var {
                    Some(var) => lw.bcx.use_var(var),
                    None => {
                        let func_ref = lw.bcx.import_func(head_closure_id);
                        let call = lw.bcx.ins().call(func_ref, &[]);
                        let v = lw.bcx.inst_results(call)[0];
                        let var = lw.bcx.declare_var(types::I64);
                        lw.bcx.def_var(var, v);
                        lw.head_closure_var = Some(var);
                        v
                    }
                };
                let same = lw.bcx.ins().icmp(IntCC::Equal, callee, head_cl);
                guard!(lw, pl, same, i, rop.pc);
            } else {
                // another function: any closure of the recorded proto runs
                // the inlined body, which reads upvalues through the frame's
                // own closure (see `frame_closure`)
                let proto = lw.bcx.ins().load(
                    types::I64,
                    cranelift_codegen::ir::MemFlagsData::trusted(),
                    callee,
                    std::mem::offset_of!(luna_core::runtime::LuaClosure, proto) as i32,
                );
                let want = emit_proto_arg(
                    &mut lw.bcx,
                    callee_proto,
                    pl.opts.aot,
                    &mut lw.defined_aot_data,
                );
                let same = lw.bcx.ins().icmp(IntCC::Equal, proto, want);
                guard!(lw, pl, same, i, rop.pc);
            }
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
            // the callee's frame: where its registers start, how many
            // arguments and extra arguments the call passes
            let shape = pl.inline_calls[i].expect("an inlined call has a frame");
            debug_assert!(
                i + 1 < effective_end,
                "inlined Call must be followed by callee op in effective_end"
            );
            let callee_base = pl.op_offsets[i + 1] as usize;
            let nparams = u32::from(callee_proto.num_params);
            if shape.n_varargs > 0 {
                // a vararg callee's extra arguments go below its registers,
                // its fixed parameters from its register 0 on (`push_frame`)
                let first = off + ins.a() as usize + 1;
                let args: Vec<Slot> = (0..shape.nargs as usize)
                    .map(|k| read_slot(lw, first + k))
                    .collect();
                for (k, v) in args.into_iter().enumerate() {
                    let dst = if (k as u32) < nparams {
                        callee_base + k
                    } else {
                        first + k - nparams as usize
                    };
                    write_slot(lw, dst, v);
                }
            }
            // a parameter the call passes no argument for starts nil
            for k in shape.nargs..nparams {
                nil_slot(lw, callee_base + k as usize);
            }
            lw.call_chain.push(FrameMaterializeInfo {
                base_offset: callee_base as u32,
                pc: rop.pc + 1,
                nresults: shape.nresults,
                n_varargs: shape.n_varargs,
            });
        }
        // a return of an inlined function: its values go to the caller's
        // R[A] on, as many as the caller wants (nil past the ones given)
        Op::Return0 | Op::Return1 | Op::Return => {
            if ins.k() {
                close_frame(lw, pl, oc)?;
            }
            let frame = lw
                .call_chain
                .pop()
                .expect("a return at depth>0 has a matching frame");
            let func = pl.frame_func[i] as usize;
            let nret = return_count(ins, pl.frame_tops[i]).expect("inline_calls fixed the count");
            let wanted = u32::try_from(frame.nresults).unwrap_or(nret);
            let vals: Vec<Slot> = (0..wanted.min(nret) as usize)
                .map(|j| read_slot(lw, off + ins.a() as usize + j))
                .collect();
            let given = vals.len();
            for (j, v) in vals.into_iter().enumerate() {
                write_slot(lw, func + j, v);
            }
            for j in given..wanted as usize {
                nil_slot(lw, func + j);
            }
            if frame.nresults < 0 {
                emit_set_top(lw, (func + nret as usize) as i64);
            }
        }
        // `...` in a vararg function the trace inlined: the frame's extra
        // arguments, below its registers, into R[A] on (nil past them)
        Op::Vararg => {
            let m = lw
                .call_chain
                .last()
                .expect("validated: a vararg expansion in an inlined frame")
                .n_varargs as usize;
            let first = pl.frame_func[i] as usize + 1;
            let want = match ins.c() {
                0 => m,
                c => c as usize - 1,
            };
            let dst = off + ins.a() as usize;
            if ins.a() as usize + want > rop.proto.max_stack as usize {
                checkpoint("bail:vararg-past-frame");
                return None;
            }
            let vals: Vec<Slot> = (0..want.min(m)).map(|j| read_slot(lw, first + j)).collect();
            let given = vals.len();
            for (j, v) in vals.into_iter().enumerate() {
                write_slot(lw, dst + j, v);
            }
            for j in given..want {
                nil_slot(lw, dst + j);
            }
            if ins.c() == 0 {
                emit_set_top(lw, (dst + m) as i64);
            }
        }
        _ => unreachable!("routed by emit_op"),
    }
    Some(())
}

/// A register's value and what the trace knows about it.
#[derive(Clone, Copy)]
pub(super) struct Slot {
    v: Value,
    kind: RegKind,
    int: Option<i64>,
    str_const: bool,
}

pub(super) fn read_slot<E: Emit>(lw: &mut Lower<E>, r: usize) -> Slot {
    Slot {
        v: lw.bcx.use_var(lw.regs_full[r]),
        kind: lw.current_kinds[r],
        int: lw.known_int[r],
        str_const: lw.const_str[r],
    }
}

pub(super) fn write_slot<E: Emit>(lw: &mut Lower<E>, r: usize, s: Slot) {
    lw.bcx.def_var(lw.regs_full[r], s.v);
    lw.current_kinds[r] = s.kind;
    lw.known_int[r] = s.int;
    lw.const_str[r] = s.str_const;
}

pub(super) fn nil_slot<E: Emit>(lw: &mut Lower<E>, r: usize) {
    let z = lw.bcx.ins().iconst(types::I64, 0);
    lw.bcx.def_var(lw.regs_full[r], z);
    lw.current_kinds[r] = RegKind::Nil;
    lw.known_int[r] = None;
    lw.const_str[r] = false;
}

/// Sets the stack top to register `rel` of the head frame, where an op
/// that takes a variable count reads it (see `luna_jit_set_top`).
pub(super) fn emit_set_top<E: Emit>(lw: &mut Lower<E>, rel: i64) {
    let f = lw.bcx.import_func(lw.h.op.set_top_id);
    let arg = lw.bcx.ins().iconst(types::I64, rel);
    lw.bcx.ins().call(f, &[arg]);
}
