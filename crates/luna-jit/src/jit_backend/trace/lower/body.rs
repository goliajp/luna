use super::*;

/// The emit pass over the body: enters the loop head and lowers each
/// recorded op up to `effective_end`.
pub(super) fn emit_body<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>) -> Option<()> {
    let Plan {
        record,
        frame_w,
        effective_end,
        active_accum,
        ..
    } = *pl;
    let Lower { body_loop, .. } = *lw;
    let RuntimeHelpers {
        str_buf_extend_id, ..
    } = lw.h.rt;
    lw.bcx.switch_to_block(body_loop);
    // Intentionally NOT sealed: the tail's clean-close back-edge
    // adds a second predecessor below.
    commit_start(lw, pl);
    // the virtual registers of an op (see `vconsts`)
    let kvars: [Variable; NVIRT] = std::array::from_fn(|_| lw.bcx.declare_var(types::I64));
    // this op's register window (a copy, so the emit code can take `lw`
    // mutably while it reads it), plus `kvars` for the virtual registers
    let mut regs_w: Vec<Variable> = Vec::with_capacity(frame_w + NVIRT);
    checkpoint("pre:main-emit-loop");
    let rewritten = rewritten_before_sync(lw.regs_full.len(), record, pl, effective_end);
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        commit_before(lw, pl, i, &rewritten[i]);
        alt_join(lw, i);
        let vregs: VRegs = pl.vconsts.get(i).copied().unwrap_or([None; NVIRT]);
        // R[C] of a register-operand op, read before this op's own write
        // forgets it (`x = x % 7` divides by the old value)
        let rc_const = match virt_at(&pl.vconsts, i, rop.inst.c(), frame_w) {
            Some(VSrc::Const(VConst::Int(n))) => Some(n),
            Some(_) => None,
            None => lw
                .known_int
                .get(pl.op_offsets[i] as usize + rop.inst.c() as usize)
                .copied()
                .flatten(),
        };
        for w in op_writes_at_offset(rop, pl.op_offsets[i]) {
            if let Some(slot) = lw.known_int.get_mut(w as usize) {
                *slot = None;
            }
            if let Some(slot) = lw.const_str.get_mut(w as usize) {
                *slot = false;
            }
        }
        // `off` is the start of this op's register
        // window inside reg_state_buf. `regs` is shadowed to the
        // matching slice of `regs_full`, so existing `regs[ins.X()]`
        // indexing auto-shifts across inlined frames. `current_kinds`
        // is NOT shadowed (mut sub-slice would block Return1's
        // cross-window write) — emit code reads/writes via the full
        // Vec with explicit `off + X` indexing.
        let off = pl.op_offsets[i] as usize;
        regs_w.clear();
        regs_w.extend_from_slice(&lw.regs_full[off..off + frame_w]);
        // the op's virtual registers, `frame_w` up (defined by
        // `enter_virt`); an op without any keeps its window exactly
        // `frame_w` wide, which the ops spilling their window rely on
        let nvirt = vregs.iter().rposition(Option::is_some).map_or(0, |j| j + 1);
        regs_w.extend_from_slice(&kvars[..nvirt]);
        let regs: &[Variable] = &regs_w;
        // body emit handler for the 4-op
        // string-accumulator idiom. Skip the 2 pre-Moves + the
        // post-Move (they're collapsed into the buffered emit).
        // Replace the Concat with `luna_jit_str_buf_extend(buf,
        // piece_raw)` + a deopt branch on -1 (piece wasn't Str
        // → existing __concat metamethod path takes over).
        if let Some(ref ba) = active_accum
            && let Some(ref fctx) = lw.flush_ctx
        {
            if i == ba.pre1_idx || i == ba.pre2_idx || Some(i) == ba.post_idx {
                continue;
            }
            if i == ba.concat_idx {
                // Read piece slot raw bits + buf ptr.
                let piece_raw = lw.bcx.use_var(regs[ba.piece_slot as usize]);
                let buf_ptr = lw.bcx.use_var(fctx.buf_var);
                let extend_ref = lw.bcx.import_func(str_buf_extend_id);
                let call_inst = lw.bcx.ins().call(extend_ref, &[buf_ptr, piece_raw]);
                let status = lw.bcx.inst_results(call_inst)[0];
                // Branch on -1 (signed less than 0) → deopt.
                let zero = lw.bcx.ins().iconst(types::I64, 0);
                let is_err = lw.bcx.ins().icmp(IntCC::SignedLessThan, status, zero);
                let continue_blk = lw.bcx.create_block();
                let deopt_blk = lw.bcx.create_block();
                lw.bcx.ins().brif(is_err, deopt_blk, &[], continue_blk, &[]);
                // Deopt path: flush buffer + store back + return pc.
                lw.bcx.switch_to_block(deopt_blk);
                lw.bcx.seal_block(deopt_blk);
                // restored with the kinds the registers have here
                guard_exit(lw, pl, rop.pc, i);
                lw.bcx.switch_to_block(continue_blk);
                lw.bcx.seal_block(continue_blk);
                continue;
            }
        }
        if pl.consumed_by_cmp[i] {
            // The cmp at i-1 already accounted for this Jmp via
            // its `brif`'s continue edge; emitting jump IR here
            // would double-jump. A closing jump still closes.
            if !matches!(rop.inst.op(), Op::JmpClose | Op::JmpCloseBack) {
                continue;
            }
        }
        // Math fold emit. Layout (see `math_folds` doc above):
        //
        //   * `Libm1` — folded indices are `start..=start+3`. The
        //     emit fires at `start` (single libm call); the trailing
        //     3 ops are silent.
        //
        //   * `Min2 / Max2` — folded indices are `start`, `start+1`,
        //     and `call_idx`. The emit fires at `call_idx` (the
        //     `Op::Call`) because that's when args have already
        //     been computed into R[A+1] / R[A+2] by the standard
        //     arg-prep ops between GetField and Call. The
        //     `start` (GetTabUp) and `start+1` (GetField) emit
        //     positions are silent — their semantic result (the
        //     resolved `math.<fn>` callable in R[A]) is known
        //     statically and never consumed by anything except
        //     the Call we're collapsing.
        let oc = OpCx {
            i,
            rop,
            vregs,
            rc_const,
            off,
            regs,
            ins: rop.inst,
            op: rop.inst.op(),
            max_stack: frame_w,
        };
        if pl.folded_ops[i] {
            emit_fold(lw, pl, &oc)?;
            continue;
        }
        // the op a failed emit stopped at, for diagnostics
        set_last_op(oc.i, oc.op as u8);
        let held = enter_virt(lw, pl, &oc, &kvars)?;
        emit_op(lw, pl, &oc)?;
        leave_virt(lw, held);
        readonly_after_op(lw, oc.op);
    }
    commit_end(lw, pl, effective_end);
    alt_join(lw, effective_end);
    debug_assert!(lw.alt_joins.is_empty(), "every skip joins a recorded op");
    Some(())
}

/// What a window slot held before an op's virtual register stood in it.
type HeldSlot = (usize, RegKind, Option<i64>, bool);

/// Defines the virtual registers of op `oc` in `kvars`, and puts their
/// kinds (and known constants) in the window slots `off + frame_w` up for
/// the op, so that the emit code reads them as it reads any register.
/// No register of a running frame is there while the op runs; the slots
/// past the widest window exist for this only. Returns what the slots
/// held.
fn enter_virt<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    kvars: &[Variable; NVIRT],
) -> Option<Vec<HeldSlot>> {
    let mut held = Vec::new();
    for (j, src) in oc.vregs.iter().enumerate() {
        let Some(src) = *src else {
            continue;
        };
        let v = match src {
            VSrc::Const(VConst::Int(n)) => lw.bcx.ins().iconst(types::I64, n),
            VSrc::Const(VConst::Float(f)) => {
                let fv = lw.bcx.ins().f64const(f);
                lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), fv)
            }
            VSrc::Const(VConst::Str(s)) => {
                emit_str_key_arg(&mut lw.bcx, s, pl.opts.aot, &mut lw.defined_aot_data)
            }
            VSrc::Const(VConst::Bool(b)) => lw.bcx.ins().iconst(types::I64, i64::from(b)),
            VSrc::Const(VConst::Nil) => lw.bcx.ins().iconst(types::I64, 0),
            VSrc::Upval(u) => upval_table_read(lw, pl, oc.i, oc.rop, u)?,
        };
        lw.bcx.def_var(kvars[j], v);
        let slot = oc.off + pl.frame_w + j;
        held.push((
            slot,
            lw.current_kinds[slot],
            lw.known_int[slot],
            lw.const_str[slot],
        ));
        lw.virt_held.push((slot, lw.current_kinds[slot]));
        lw.current_kinds[slot] = vsrc_kind(src);
        lw.known_int[slot] = match src {
            VSrc::Const(VConst::Int(n)) => Some(n),
            _ => None,
        };
        lw.const_str[slot] = matches!(src, VSrc::Const(VConst::Str(_)));
    }
    Some(held)
}

/// Puts back what [`enter_virt`] found in the window slots.
fn leave_virt<E: Emit>(lw: &mut Lower<E>, held: Vec<HeldSlot>) {
    for (slot, kind, int, s) in held {
        lw.current_kinds[slot] = kind;
        lw.known_int[slot] = int;
        lw.const_str[slot] = s;
    }
    lw.virt_held.clear();
}
