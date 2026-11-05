use super::*;

#[macro_use]
mod macros;
mod alt;
mod begin;
mod body;
mod clif_driver;
mod commit;
mod downrec_tail;
mod exit;
mod finish;
mod fold;
mod fold_calls;
mod fold_fmod;
mod gc_due;
mod gc_roots;
mod handoff;
mod helpers;
mod loop_tail;
mod ops;
mod peel;
mod plan;
mod prologue;
mod readonly;
mod state;
mod step_guard;
mod sunk_sites;
mod tail;
mod tfor_tail;
use alt::*;
use begin::*;
use body::*;
use clif_driver::*;
pub(super) use commit::ops_all_pure;
use commit::*;
use downrec_tail::*;
use exit::*;
use finish::*;
use fold::*;
use fold_calls::*;
use fold_fmod::*;
use gc_due::*;
use gc_roots::*;
use handoff::*;
pub(in crate::jit_backend::trace) use helpers::Helpers;
use helpers::*;
use loop_tail::*;
use ops::*;
use peel::*;
use plan::*;
use prologue::*;
use readonly::*;
use state::{Known, Lower, UpvalReads, entry_values};
use step_guard::*;
use sunk_sites::*;
use tail::*;
use tfor_tail::*;

/// `always_codegen = false` leaves the function undefined in `module`
/// when [`trace_is_enterable`] says nothing will run it; `float_only` as
/// in [`compile_trace_jit`].
pub(super) fn lower_trace_into_inner<M: Module>(
    module: &mut M,
    record: &TraceRecord,
    opts: CompileOptions,
    aot_fn_name: Option<&str>,
    always_codegen: bool,
    float_only: bool,
    at_exits: bool,
) -> Option<(FuncId, CompiledTraceParts)> {
    with_plan(record, opts, float_only, |pl, escape| {
        lower_clif(module, pl, escape, aot_fn_name, always_codegen, at_exits)
    })?
}

/// Plans `record` and hands the plan to `f`; `None` when the record
/// cannot be lowered.
fn with_plan<R>(
    record: &TraceRecord,
    opts: CompileOptions,
    float_only: bool,
    f: impl FnOnce(&Plan<'_>, EscapeAnalysis) -> R,
) -> Option<R> {
    checkpoint("enter");
    if !record.closed {
        checkpoint("bail:not-closed");
        return None;
    }
    checkpoint("post:closed-check");
    let head_proto = record.head_proto;
    let max_stack = head_proto.max_stack as usize;
    // every op sees a register window this wide: the largest frame among
    // the functions the trace runs (the head's and any it inlined)
    let frame_w = record
        .ops
        .iter()
        .map(|r| r.proto.max_stack as usize)
        .fold(max_stack, usize::max);
    // Every pass below reads register operands: a constant- or
    // immediate-operand op is lowered as its register form with the
    // constant in virtual register `frame_w` (one past the widest frame,
    // never stored back), whose kind and value `vconsts` holds.
    let translated;
    let (record, vconsts) = match split_const_operands(record, frame_w as u32) {
        Some((t, v)) => {
            translated = Some(t);
            (translated.as_ref().unwrap_or(record), v)
        }
        None => {
            translated = None;
            (record, Vec::new())
        }
    };
    let out = 'planned: {
        let Some((plan, escape)) = plan_trace(
            record, vconsts, head_proto, max_stack, frame_w, opts, float_only,
        ) else {
            break 'planned None;
        };
        // a root trace reading a register on entry that holds a value no
        // trace is entered with (a boolean, a coroutine) could never run: it
        // is not compiled, and leaves its head free for a later recording
        let never_entered = record.side_trace_parent.is_none()
            && record
                .entry_tags
                .iter()
                .zip(&plan.head_live)
                .any(|(&t, &live)| live && !luna_core::jit::trace::entry_tag_enterable(t));
        if never_entered {
            checkpoint("bail:entry-tag-never-entered");
            break 'planned None;
        }
        Some(f(&plan, escape))
    };
    if let Some(t) = translated {
        give_record(t);
    }
    out
}

/// The trace recorded for the baseline code generator, with what the
/// emit pass decided; `None` when it cannot be lowered. `at_exits`: see
/// `Lower::at_exits`.
pub(super) fn lower_trace_lir(
    record: &TraceRecord,
    opts: CompileOptions,
    float_only: bool,
    at_exits: bool,
) -> Option<(super::lir::Lir, CompiledTraceParts)> {
    with_plan(record, opts, float_only, |pl, escape| {
        let mut e = super::lir::Lir::take();
        let h = match e.helpers {
            Some((h, _, _)) => h,
            None => {
                let h = declare_helpers(&mut e)?;
                e.helpers = Some((h, e.funcs.len() as u32, e.param_tys.len() as u32));
                h
            }
        };
        let count_at = match opts.tier {
            TraceTier::Auto => opts.tier_up_at,
            _ => 0,
        };
        let (e, emitted) = emit_trace(e, pl, h, escape, count_at, at_exits)?;
        Some((e, build_compiled(pl, emitted)))
    })?
}

/// Emits the whole trace through `bcx`: the entry block, the body and the
/// tail. Returns the builder with what the emit pass decided.
/// `count_at > 0` keeps an iteration count at the back edge (see
/// [`Emit::tier_count`]).
fn emit_trace<E: Emit>(
    mut bcx: E,
    pl: &Plan<'_>,
    h: Helpers,
    escape: EscapeAnalysis,
    count_at: u32,
    at_exits: bool,
) -> Option<(E, Emitted)> {
    let head = emit_entry(&mut bcx, pl);
    // a variable: `loop_entry` reads the address again (see `handoff`)
    let due_words = emit_due_words(&mut bcx, pl)?.map(|w| {
        let var = bcx.declare_var(types::I64);
        bcx.def_var(var, w);
        var
    });
    let mut escape = escape;
    let sunk = alloc_sunk_sites(&mut bcx, pl, &mut escape);
    let flush_ctx = start_accum(&mut bcx, pl, h, &head.regs_full);
    let blocks = open_body_loop(&mut bcx, pl);
    let mut lower = begin_body(bcx, pl, h, head, escape, sunk, flush_ctx, blocks);
    if count_at > 0 {
        let calls_at = pl.record.head_proto.call_hot_count();
        lower.tier_count = Some(Box::new(TierUp::new(
            Box::new(TCellU32::new(0)),
            count_at,
            calls_at,
        )));
    }
    lower.due_words = due_words;
    // only a body of pure ops gains: elsewhere every op that is not pure
    // commits everything anyway, and the exits store more
    lower.at_exits = at_exits && (0..pl.effective_end).all(|i| !syncs_before(pl.record, pl, i));
    let lw = &mut lower;
    emit_fold_precheck(lw, pl);
    emit_readonly_precheck(lw, pl);
    emit_step_precheck(lw, pl);
    let loop_copy = emit_body(lw, pl)?;
    let downrec = emit_tail(lw, pl)?;
    if let Some(lc) = loop_copy {
        emit_loop_copy(lw, pl, lc)?;
    }
    // last, so that it takes nothing from the blocks lowered before it
    emit_loop_entry(lw)?;
    if !exits_fit(lower.per_exit_inline_vec.len(), lower.per_exit_kinds.len()) {
        checkpoint("exits:too-many");
        return None;
    }
    Some(lower.into_emitted(downrec))
}

/// Whether `inline` inline sites (returned as their number + 1) and
/// `tagged` tagged exits fit the bits of a return value that number them.
fn exits_fit(inline: usize, tagged: usize) -> bool {
    let limit = luna_core::__private::EXIT_SITE_LIMIT as usize;
    inline < limit && tagged <= limit
}

#[cfg(test)]
mod tests {
    use super::exits_fit;
    use luna_core::__private::EXIT_SITE_LIMIT;

    #[test]
    fn exit_numbers_up_to_the_site_field_compile() {
        let l = EXIT_SITE_LIMIT as usize;
        assert!(exits_fit(l - 1, l));
        assert!(!exits_fit(l, 0));
        assert!(!exits_fit(0, l + 1));
    }
}
