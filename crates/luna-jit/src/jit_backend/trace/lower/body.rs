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
    let Lower {
        reg_state,
        body_loop,
        ..
    } = *lw;
    let RuntimeHelpers {
        str_buf_extend_id, ..
    } = lw.h.rt;
    let vconst = |i: usize| pl.vconsts.get(i).copied().flatten();
    lw.bcx.switch_to_block(body_loop);
    // Intentionally NOT sealed: the tail's clean-close back-edge
    // adds a second predecessor below.
    lw.stored.extend(
        lw.regs_full
            .iter()
            .map(|&v| Some(use_var_resolved(&mut lw.bcx, v))),
    );
    // the virtual register of a constant-operand op (see `vconsts`)
    let kvar = lw.bcx.declare_var(types::I64);
    // this op's register window (a copy, so the emit code can take `lw`
    // mutably while it reads it), plus `kvar` for a constant operand
    let mut regs_w: Vec<Variable> = Vec::with_capacity(frame_w + 1);
    checkpoint("pre:main-emit-loop");
    for (i, rop) in record.ops[..effective_end].iter().enumerate() {
        // Commit the earlier ops' register writes to reg_state where this
        // op may read it, and where it can take another way that rejoins
        // later (both ways then hold the same reg_state)
        if !pure_op(rop.inst.op()) || pl.alt_paths.get(i).is_some_and(|a| a.is_some()) {
            sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
        }
        alt_join(lw, i);
        let vk = vconst(i);
        // R[C] of a register-operand op, read before this op's own write
        // forgets it (`x = x % 7` divides by the old value)
        let rc_const = match vk {
            Some(k) if rop.inst.c() as usize == frame_w => match k {
                VConst::Int(n) => Some(n),
                VConst::Float(_) => None,
            },
            _ => lw
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
        // a constant operand: its value in `kvar`, which `regs` gets as
        // register `frame_w`
        if let Some(k) = vk {
            let v = match k {
                VConst::Int(n) => lw.bcx.ins().iconst(types::I64, n),
                VConst::Float(f) => {
                    let fv = lw.bcx.ins().f64const(f);
                    lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), fv)
                }
            };
            lw.bcx.def_var(kvar, v);
            regs_w.push(kvar);
        }
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
            if i == ba.pre1_idx || i == ba.pre2_idx || i == ba.post_idx {
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
            // would double-jump.
            continue;
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
            vk,
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
        emit_op(lw, pl, &oc)?;
        readonly_after_op(lw, oc.op);
    }
    // the tails that call helpers reading reg_state; the others store
    // what they leave with
    let generic_for = pl
        .for_loop_idx_opt
        .is_some_and(|k| record.ops[k].inst.op() == Op::TForLoop);
    if generic_for || pl.downrec_idx_opt.is_some() || pl.self_link_idx_opt.is_some() {
        sync_reg_state(&mut lw.bcx, &lw.regs_full, &mut lw.stored, reg_state);
    }
    alt_join(lw, effective_end);
    debug_assert!(lw.alt_joins.is_empty(), "every skip joins a recorded op");
    Some(())
}

/// Ops whose lowering neither calls a helper that reads reg_state or the
/// Lua stack nor lets the collector run: arithmetic, comparisons, moves,
/// loads of constants and the numeric `for` step. Their guards leave
/// through exits, which store what they leave with.
fn pure_op(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::LoadI
            | Op::LoadF
            | Op::LoadK
            | Op::LoadNil
            | Op::LoadFalse
            | Op::LoadTrue
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::IDiv
            | Op::Mod
            | Op::AddI
            | Op::AddK
            | Op::SubK
            | Op::MulK
            | Op::DivK
            | Op::IDivK
            | Op::ModK
            | Op::Unm
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr
            | Op::BAndK
            | Op::BOrK
            | Op::BXorK
            | Op::ShrI
            | Op::ShlI
            | Op::BNot
            | Op::Not
            | Op::Lt
            | Op::Le
            | Op::Eq
            | Op::LtI
            | Op::LeI
            | Op::GtI
            | Op::GeI
            | Op::EqI
            | Op::EqK
            | Op::Test
            | Op::Jmp
            | Op::ForLoop
    )
}
