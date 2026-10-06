use super::*;

mod cmp_table_checks;
mod frames;
mod op_checks;
mod scan;
mod validate;
use cmp_table_checks::*;
use frames::*;
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
    /// The width of every op's register window: the largest frame among
    /// the functions the trace runs. Register `frame_w` is the virtual one
    /// of a constant operand.
    pub(super) frame_w: usize,
    pub(super) vconsts: Vec<Option<VConst>>,
    pub(super) opts: CompileOptions,
    pub(super) float_only: bool,
    pub(super) op_offsets: Vec<u32>,
    /// The frame of each call the trace inlines (see [`inline_calls`]).
    pub(super) inline_calls: Vec<Option<InlineCall>>,
    /// The stack top of each op's frame before it runs, where the
    /// recording fixes it.
    pub(super) frame_tops: Vec<Option<u32>>,
    /// The register holding the closure each op's frame runs.
    pub(super) frame_func: Vec<u32>,
    /// The registers each op writes past what its instruction names (see
    /// [`inline_writes`]).
    pub(super) inline_writes: Vec<(u32, u32)>,
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
    /// A comparison's other way, taken in the trace. Its moves and loads
    /// leave each register the kind the recorded way leaves, so in 5.1 /
    /// 5.2 no integer reaches arithmetic the recorder did not check.
    pub(super) alt_paths: Vec<Option<alt_path::AltPath>>,
    /// The step register of a 5.3 integer loop and its recorded sign, when
    /// the trace checks the sign once (see `step_guard`).
    pub(super) step_guard: Option<(usize, bool)>,
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
    frame_w: usize,
    opts: CompileOptions,
    float_only: bool,
) -> Option<(Plan<'r>, EscapeAnalysis)> {
    let n = record.ops.len();
    set_last_op(usize::MAX, 255);

    // a trace that inlines no call needs none of the inline-frame plan
    // (the vectors stay empty; their readers take a missing entry as none)
    let inlines = record.ops.iter().any(|r| r.inline_depth > 0);
    let (inline_calls, frame_tops) = if inlines {
        inline_calls(record)
    } else {
        (Vec::new(), Vec::new())
    };
    let (op_offsets, window_size) = plan_frames(record, head_proto, frame_w, &inline_calls)?;
    let window_size_us = window_size as usize;
    let (frame_func, inline_writes) = if inlines {
        let funcs = frame_funcs(record, &op_offsets);
        let writes = inline_writes(record, &op_offsets, &inline_calls, &frame_tops, &funcs);
        (funcs, writes)
    } else {
        (Vec::new(), Vec::new())
    };

    side_trace_gate(record, &op_offsets)?;
    let (folded_ops, math_folds) = scan_math_folds(record, n, head_proto, opts);
    let end_idx_opt = find_trace_end(record, &folded_ops, n, &inline_calls)?;
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
        frame_w,
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
    let alt_paths = alt_path::find_alt_paths(record, effective_end, head_proto);
    let mut head_live = entry_live(
        record,
        &op_offsets,
        &inline_writes,
        effective_end,
        max_stack,
        do_internal_loop,
        parent_exit_tags.as_deref(),
    );
    for r in alt_path::alt_written(&alt_paths) {
        if let Some(l) = head_live.get_mut(r as usize) {
            *l = true;
        }
    }
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
        frame_w,
        effective_end,
        &folded_ops,
        &frame_tops,
    )?;
    let step_guard = plan_step_guard(
        record,
        &op_offsets,
        &head_live,
        &alt_paths,
        for_loop_idx_opt,
        opts.pre53 && !float_only,
    );
    validate_trace_ends(
        record,
        head_proto,
        max_stack,
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
            frame_w,
            vconsts,
            opts,
            float_only,
            op_offsets,
            inline_calls,
            frame_tops,
            frame_func,
            inline_writes,
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
            alt_paths,
            step_guard,
        },
        escape,
    ))
}
