use super::*;

mod cmp_table_checks;
mod op_checks;
mod scan;
mod validate;
use cmp_table_checks::*;
use op_checks::*;
use scan::*;
use validate::*;

// detect the FIRST Bufferable AccumSite.
// Buffered emit handles a single site. The 4 idiom op indices
// are pre1 = op_idx-2, pre2 = op_idx-1, concat = op_idx,
// post = op_idx+1.
#[derive(Clone, Copy, Debug)]
pub(super) struct BufferedAccum {
    pub(super) accum_slot: u32,
    pub(super) piece_slot: u32,
    pub(super) pre1_idx: usize,
    pub(super) pre2_idx: usize,
    pub(super) concat_idx: usize,
    pub(super) post_idx: usize,
}

/// What the pre-emit passes found out about a trace: its register
/// windows, where it ends, which ops are folded or consumed, and the
/// shape of its tail. Read-only once built.
pub(super) struct Plan<'r> {
    pub(super) record: &'r TraceRecord,
    pub(super) head_proto: Gc<Proto>,
    pub(super) max_stack: usize,
    pub(super) vconsts: Vec<Option<VConst>>,
    pub(super) opts: CompileOptions,
    pub(super) float_only: bool,
    pub(super) op_offsets: Vec<u32>,
    pub(super) enclosing_call_a: Vec<Option<u8>>,
    pub(super) window_size: u32,
    pub(super) window_size_us: usize,
    pub(super) folded_ops: Vec<bool>,
    pub(super) math_folds: Vec<TraceMathFold>,
    pub(super) end_idx_opt: Option<(usize, TraceEnd)>,
    pub(super) effective_end: usize,
    pub(super) call_idx_opt: Option<usize>,
    pub(super) for_loop_idx_opt: Option<usize>,
    pub(super) inline_abort_idx_opt: Option<usize>,
    pub(super) return_idx_opt: Option<usize>,
    pub(super) self_link_idx_opt: Option<(usize, SelfRecKind)>,
    pub(super) downrec_idx_opt: Option<(usize, u32, usize, u8)>,
    pub(super) do_internal_loop: bool,
    pub(super) head_live: Vec<bool>,
    pub(super) active_accum: Option<BufferedAccum>,
    pub(super) consumed_by_cmp: Vec<bool>,
    pub(super) cmp_dirs: Vec<Option<CmpDir>>,
}

/// The constant held by the virtual register of op `i`, if it has one.
pub(super) fn vconst_at(vconsts: &[Option<VConst>], i: usize) -> Option<VConst> {
    vconsts.get(i).copied().flatten()
}

/// The pre-emit passes. `None` when the trace cannot be compiled; the
/// escape analysis comes back separately because emit demotes sites.
pub(super) fn plan_trace<'r>(
    record: &'r TraceRecord,
    vconsts: Vec<Option<VConst>>,
    head_proto: Gc<Proto>,
    max_stack: usize,
    opts: CompileOptions,
    float_only: bool,
) -> Option<(Plan<'r>, EscapeAnalysis)> {
    let n = record.ops.len();

    let (op_offsets, enclosing_call_a, window_size) = plan_frames(record, head_proto, max_stack)?;
    let window_size_us = window_size as usize;

    side_trace_gate(record, &op_offsets)?;
    validate_inline_calls(record, head_proto)?;
    let (folded_ops, math_folds) = scan_math_folds(record, n, head_proto, opts);
    let end_idx_opt = find_trace_end(record, &folded_ops, head_proto, n)?;
    let effective_end = end_idx_opt.map(|(i, _)| i).unwrap_or(n);
    // escape analysis over the recorded body +
    // terminator. The pre-emit pass below demotes any Sinkable
    // site that doesn't meet the sunk-emit criteria back to
    // Escaped, so emit only honours sites we actually allocate
    // virt-slot Variables for.
    checkpoint("post:end-idx-found");
    let escape = escape_analyze(
        record,
        effective_end,
        end_idx_opt.map(|(_, k)| k),
        head_proto,
    );
    checkpoint("post:escape-analyze");
    // Keep the old names working for the per-tail paths below.
    let call_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::Call)) => Some(i),
        _ => None,
    };
    let for_loop_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::ForLoop)) => Some(i),
        _ => None,
    };
    let inline_abort_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::InlineAbort)) => Some(i),
        _ => None,
    };
    let return_idx_opt = match end_idx_opt {
        Some((i, TraceEnd::Return)) => Some(i),
        _ => None,
    };
    // `self_link_idx_opt = Some(effective_end)` when this is a
    // self-link close. Used to gate the new tail emit arm + override
    // `do_internal_loop` (the trace is designed to loop).
    let self_link_idx_opt: Option<(usize, SelfRecKind)> = match end_idx_opt {
        Some((i, TraceEnd::SelfLink(kind))) => Some((i, kind)),
        _ => None,
    };
    // `downrec_idx_opt` = `Some(effective_end, return_pc,
    // target_proto_id, depth_delta)` when the down-rec catch tripped.
    let downrec_idx_opt: Option<(usize, u32, usize, u8)> = match end_idx_opt {
        Some((
            i,
            TraceEnd::DownRec {
                return_pc,
                target_proto_id,
                depth_delta,
            },
        )) => Some((i, return_pc, target_proto_id, depth_delta)),
        _ => None,
    };

    let has_cmp = record.ops[..effective_end]
        .iter()
        .any(|r| matches!(r.inst.op(), Op::Lt | Op::Le | Op::Eq));
    // SelfLink close ALSO permits internal loop (in fact it
    // REQUIRES it — the whole point of the trace is to loop with
    // bump-base + branch-to-self). Treat self_link_idx_opt the same as
    // the cmp/ForLoop loop-permission predicates, and (critically)
    // don't trip on inline_abort_idx_opt — the SelfLink close path
    // doesn't go through that arm.
    // `downrec_idx_opt` does NOT permit internal loop: its tail
    // either stitches through the dispatcher or deopts, so DownRec
    // is a hard truncation marker that forces one-shot dispatch.
    let do_internal_loop = opts.internal_loop
        && (has_cmp || for_loop_idx_opt.is_some() || self_link_idx_opt.is_some())
        && call_idx_opt.is_none()
        && return_idx_opt.is_none()
        && inline_abort_idx_opt.is_none()
        && downrec_idx_opt.is_none();
    // The head-frame registers the dispatcher checks on entry (see
    // `entry_live`); the others start held on the stack.
    let parent_exit_tags = side_parent_exit_tags(record);
    let head_live = entry_live(
        record,
        &op_offsets,
        effective_end,
        max_stack,
        do_internal_loop,
        parent_exit_tags.as_deref(),
    );
    let active_accum: Option<BufferedAccum> = escape
        .accum_sites
        .iter()
        .find(|s| s.state == BufferState::Bufferable)
        // the buffer reads both as strings on the entry check's word
        .filter(|s| head_live[s.accum_slot as usize] && head_live[s.piece_slot as usize])
        .map(|s| BufferedAccum {
            accum_slot: s.accum_slot,
            piece_slot: s.piece_slot,
            pre1_idx: s.op_idx - 2,
            pre2_idx: s.op_idx - 1,
            concat_idx: s.op_idx,
            post_idx: s.op_idx + 1,
        });

    let (consumed_by_cmp, cmp_dirs) = validate_ops(
        record,
        &vconsts,
        head_proto,
        max_stack,
        effective_end,
        &folded_ops,
    )?;
    validate_trace_ends(
        record,
        head_proto,
        max_stack,
        opts,
        effective_end,
        &consumed_by_cmp,
        call_idx_opt,
        return_idx_opt,
        for_loop_idx_opt,
    )?;
    Some((
        Plan {
            record,
            head_proto,
            max_stack,
            vconsts,
            opts,
            float_only,
            op_offsets,
            enclosing_call_a,
            window_size,
            window_size_us,
            folded_ops,
            math_folds,
            end_idx_opt,
            effective_end,
            call_idx_opt,
            for_loop_idx_opt,
            inline_abort_idx_opt,
            return_idx_opt,
            self_link_idx_opt,
            downrec_idx_opt,
            do_internal_loop,
            head_live,
            active_accum,
            consumed_by_cmp,
            cmp_dirs,
        },
        escape,
    ))
}

/// The record's frame shape: per-op register-window offsets, the
/// enclosing call's A per op, and the window size.
fn plan_frames(
    record: &TraceRecord,
    head_proto: Gc<Proto>,
    max_stack: usize,
) -> Option<(Vec<u32>, Vec<Option<u8>>, u32)> {
    // recorder invariant: the first recorded op is at
    // depth 0 on `head_proto`. A record violating either would break
    // `compute_op_offsets`' depth-bump arithmetic; bail cleanly here
    // rather than panic deeper in.
    if let Some(first) = record.ops.first()
        && (first.inline_depth != 0 || !std::ptr::eq(first.proto.as_ptr(), head_proto.as_ptr()))
    {
        checkpoint("bail:first-op-shape");
        return None;
    }
    checkpoint("post:first-op-check");

    // The smart side-trace gate sits BELOW `compute_op_offsets` so
    // it can reuse the verified op_offsets (calling
    // compute_op_offsets early can panic if the depth invariant
    // fails — verify_depth_invariant runs first).

    // per-op register-window offsets across inlined
    // self-recursive frames. `op_offsets[i]` is the start of op i's
    // register window inside reg_state_buf; `enclosing_call_a[i]` is
    // the matching caller `Op::Call`'s A field (None at depth 0).
    // `window_size` is the largest `off + max_stack` across all ops —
    // sized so even the deepest inlined frame fits. The dispatcher
    // (vm/exec.rs) reads `window_size` off `CompiledTrace` to size
    // its reg_state buffer; only [0..max_stack) is marshalled in
    // from the interp stack, [max_stack..window_size) is zero-init
    // and filled by the trace's own GetUpval / arith.
    // consolidated depth invariant check. Bails if
    // any of:
    //   - first op not at depth 0 (already checked above against
    //     head_proto, but kept here for the pure-function test)
    //   - any consecutive ops jump > 1 depth (e.g. Op::Close
    //     pushing both a Cont::Close frame AND a handler's Lua
    //     frame — recorder sees 0 → 2, IR has no intermediate)
    //   - a depth bump is not preceded by an Op::Call (recorder
    //     contract: only Op::Call can push a new frame)
    //   - any op exceeds MAX_INLINE_DEPTH (the lowerer caps its
    //     window_size on this)
    // The check is pulled into `verify_depth_invariant` (lib
    // unit tested over synthetic depth/Op-is-Call sequences;
    // doesn't need a real `Gc<Proto>`).
    let depth_items: Vec<(u8, bool)> = record
        .ops
        .iter()
        .map(|r| (r.inline_depth, matches!(r.inst.op(), Op::Call)))
        .collect();
    if !verify_depth_invariant(&depth_items) {
        checkpoint("bail:depth-invariant");
        return None;
    }
    checkpoint("post:depth-invariant");
    let (op_offsets, enclosing_call_a) = compute_op_offsets(record);
    let mut window_size: u32 = op_offsets
        .iter()
        .map(|&off| off + max_stack as u32)
        .max()
        .unwrap_or(max_stack as u32);
    // SelfLink close needs `regs_full` to extend through the
    // would-be-next-depth's window so the snapshot-restore copy reads
    // from valid slots. Without this extension, compute_op_offsets
    // only covers the deepest CAPTURED depth (the recorder closed
    // BEFORE pushing the tripping depth's frame), and bump-target
    // reads would go OOB. Extend by one max_stack window past the
    // last Op::Call's bump destination.
    if record.self_link_kind.is_some() {
        let mut last_call_idx: Option<usize> = None;
        for (i, rop) in record.ops.iter().enumerate() {
            if matches!(rop.inst.op(), Op::Call) {
                last_call_idx = Some(i);
            }
        }
        if let Some(idx) = last_call_idx {
            let bump_off = op_offsets[idx] + record.ops[idx].inst.a() + 1;
            let needed = bump_off + max_stack as u32;
            if needed > window_size {
                window_size = needed;
            }
        }
    }
    Some((op_offsets, enclosing_call_a, window_size))
}

fn side_trace_gate(record: &TraceRecord, op_offsets: &[u32]) -> Option<()> {
    // SMART side-trace gate. Compute the child's read-before-write live-
    // in slot set (slots READ without first being WRITTEN within
    // child's body — values carried in from the parent's exit
    // reg_state). Intersect with the parent's body_writes (slots
    // the parent's recorded body writes — values that go STALE
    // across child's internal-loop iters because parent doesn't
    // re-run those writes mid-side-trace). Non-empty intersection
    // = the s12_step_b class of bug; bail compile. Empty = side
    // trace is self-contained w.r.t. parent's writes — safe to
    // internal-loop OR forward-only — allow either.
    //
    // More permissive than banning ALL back-edge ops in side
    // traces: self-contained back-edge side traces (e.g. recursive
    // call branches that re-compute their inputs each iter) can
    // compile and
    // amortize the parent's hot-exit dispatch cost.
    if let Some((parent_proto, parent_head_pc, _)) = record.side_trace_parent {
        // Check 1: any back-edge op? (ForLoop / TForLoop / Jmp -bx)
        let has_back_edge = record.ops.iter().any(|op| match op.inst.op() {
            luna_core::vm::isa::Op::ForLoop | luna_core::vm::isa::Op::TForLoop => true,
            luna_core::vm::isa::Op::Jmp => op.inst.sbx() < 0,
            _ => false,
        });
        if has_back_edge {
            // Back-edge means the trace's IR will internal-loop OR
            // re-execute body ops. Two correctness requirements:
            //
            //   (a) child must not READ a slot the parent's body
            //       writes without first writing it itself (the
            //       s12_step_b stale-register bug).
            //   (b) child must not contain side-effect-producing
            //       ops (Call / TForCall / SetTable / SetI /
            //       SetField / SetUpval / SetTabUp / Closure /
            //       Close / Tbc) — these advance shared heap /
            //       iterator state that interp re-observes after
            //       the side trace returns, causing double-effect
            //       (the s12_step_d TForCall-double-advance bug).
            let has_impure = record.ops.iter().any(|op| {
                use luna_core::vm::isa::Op;
                matches!(
                    op.inst.op(),
                    Op::Call
                        | Op::TailCall
                        | Op::TForCall
                        | Op::SetTable
                        | Op::SetI
                        | Op::SetField
                        | Op::SetUpval
                        | Op::SetTabUp
                        | Op::Closure
                        | Op::Close
                        | Op::Tbc
                )
            });
            if has_impure {
                checkpoint("bail:side-trace-back-edge-with-impure");
                return None;
            }
            // Pure back-edge trace: still check live-in vs parent
            // writes (Add/Move loops can still re-read a stale
            // parent-written slot each iter).
            let child_live_in = compute_live_in_slots(record, op_offsets);
            if !child_live_in.is_empty() {
                let parent_writes_opt = {
                    let traces = parent_proto.traces.borrow();
                    traces
                        .iter()
                        .find(|t| t.head_pc == parent_head_pc)
                        .map(|pct| pct.body_writes.clone())
                };
                if let Some(parent_writes) = parent_writes_opt {
                    let mut i = 0;
                    let mut j = 0;
                    let pw = &parent_writes[..];
                    let cl_li = &child_live_in[..];
                    while i < pw.len() && j < cl_li.len() {
                        match pw[i].cmp(&cl_li[j]) {
                            std::cmp::Ordering::Equal => {
                                checkpoint("bail:side-trace-live-in-overlap");
                                return None;
                            }
                            std::cmp::Ordering::Less => i += 1,
                            std::cmp::Ordering::Greater => j += 1,
                        }
                    }
                } else {
                    checkpoint("bail:side-trace-parent-ct-missing");
                    return None;
                }
            }
            // All back-edge checks passed; trace is allowed.
        }
        // No back-edge: forward-only is always safe (single-iter
        // execution; no internal-loop semantics to break).
    }
    checkpoint("post:side-trace-v2e-smart-gate");
    Some(())
}
