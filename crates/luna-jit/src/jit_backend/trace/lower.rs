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
mod gc_roots;
mod helpers;
mod loop_tail;
mod ops;
mod plan;
mod prologue;
mod readonly;
mod step_guard;
mod sunk_sites;
mod tail;
mod tfor_tail;
use alt::*;
use begin::*;
use body::*;
use clif_driver::*;
use commit::*;
use downrec_tail::*;
use exit::*;
use finish::*;
use fold::*;
use fold_calls::*;
use fold_fmod::*;
use gc_roots::*;
pub(in crate::jit_backend::trace) use helpers::Helpers;
use helpers::*;
use loop_tail::*;
use ops::*;
use plan::*;
use prologue::*;
use readonly::*;
use step_guard::*;
use sunk_sites::*;
use tail::*;
use tfor_tail::*;

/// The trace function under construction and everything the emit pass
/// tracks while lowering it.
struct Lower<E: Emit> {
    bcx: E,
    h: Helpers,
    reg_state: Value,
    trace_fn_sig_ref: cranelift_codegen::ir::SigRef,
    global_side_trace_box: Box<TCellPtr>,
    regs_full: Vec<Variable>,
    tforcall_tag_var: Variable,
    tforcall_val_tag_var: Variable,
    precheck: Option<Block>,
    /// The block before the loop head that tests the tables
    /// `ro_invariant` names (see `readonly`).
    ro_precheck: Option<Block>,
    /// The block before the loop head that checks a 5.3 loop's step sign.
    step_precheck: Option<Block>,
    body_loop: Block,
    head_kinds: Vec<RegKind>,
    defined_aot_data: std::collections::HashSet<DataId>,
    escape: EscapeAnalysis,
    flush_ctx: Option<FlushCtx>,
    virt_vars: Vec<Option<Vec<Variable>>>,
    virt_kinds: Vec<Option<Vec<RegKind>>>,
    sunk_alloc_seen: u32,
    materialize_emit_count: u32,
    closure_seen: u32,
    stored: Vec<Option<Value>>,
    /// Commit register writes to reg_state only where something reads it
    /// (`commit.rs`), keeping values in machine registers across the back
    /// edge: for the optimizing tiers, whose register allocators make use
    /// of that; the baseline tier's would spill.
    at_exits: bool,
    /// With `at_exits`, per register: the value reg_state holds for it at
    /// the loop head whichever way the head is reached (`None`: the body
    /// writes the register and the back edge does not store it).
    head_stored: Vec<Option<Value>>,
    current_kinds: Vec<RegKind>,
    dispatchable: bool,
    dispatch_off_reason: Option<&'static str>,
    per_exit_kinds: ExitKinds,
    per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )>,
    call_chain: Vec<FrameMaterializeInfo>,
    upval_cache: std::collections::HashMap<u32, Variable>,
    /// Per upvalue index, the checked typed read: its variable, out slot,
    /// check result and type.
    upval_checked:
        std::collections::HashMap<u32, (Variable, cranelift_codegen::ir::StackSlot, Value, u8)>,
    /// The upvalues whose checked read has been guarded.
    upval_check_done: Vec<u32>,
    head_closure_var: Option<Variable>,
    known_int: Vec<Option<i64>>,
    /// The registers holding a string constant loaded earlier in the same
    /// pass: a table access by such a key may use the slot the recording
    /// found it in.
    const_str: Vec<bool>,
    /// The window slots the virtual registers of the op being lowered
    /// stand in, with the kinds the slots had (see `enter_virt`): an exit
    /// taken inside the op snapshots those kinds, not the virtual ones.
    virt_held: Vec<(usize, RegKind)>,
    /// Blocks the other way of a comparison jumps to, by the recorded op
    /// it rejoins at, with the registers the skipped ops write.
    alt_joins: std::collections::HashMap<usize, (Block, Vec<u32>)>,
    /// The head-frame registers whose table is tested before the loop head
    /// instead of at each store (see `readonly`).
    ro_invariant: Vec<bool>,
    /// The table values tested since the last op that may have run host
    /// code.
    ro_checked: Vec<Value>,
    /// The iteration count the back edge keeps for tiering up, and the
    /// count to leave at.
    tier_count: Option<(Box<TCellU32>, u32)>,
}

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
) -> Option<(FuncId, CompiledTrace)> {
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

/// Whether every op of `record` is one the optimizing tiers lower with
/// values kept to the exits (`Lower::at_exits`); for any other record
/// lowering again gives the baseline tier's instructions.
pub(super) fn ops_all_pure(record: &TraceRecord) -> bool {
    record.ops.iter().all(|r| commit::pure_op(r.inst.op()))
}

/// The trace recorded for the baseline code generator, with what the
/// emit pass decided; `None` when it cannot be lowered. `at_exits`: see
/// `Lower::at_exits`.
pub(super) fn lower_trace_lir(
    record: &TraceRecord,
    opts: CompileOptions,
    float_only: bool,
    at_exits: bool,
) -> Option<(super::lir::Lir, CompiledTrace)> {
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
    // track which AOT data slots
    // we've already `define_data`'d this lower call. `declare_data`
    // returns the same `DataId` for the same name (Cranelift name
    // interning), but `define_data` rejects redefinition with
    // `ModuleError::DuplicateDefinition` — so the dedupe guard sits
    // around `define_data`, not `declare_data`.
    let defined_aot_data: std::collections::HashSet<DataId> = std::collections::HashSet::new();
    let head = emit_entry(&mut bcx, pl);
    let mut escape = escape;
    let sunk = alloc_sunk_sites(&mut bcx, pl, &mut escape);
    let flush_ctx = start_accum(&mut bcx, pl, h, &head.regs_full);
    let blocks = open_body_loop(&mut bcx, pl);
    let mut lower = begin_body(
        bcx,
        pl,
        h,
        head,
        escape,
        defined_aot_data,
        sunk,
        flush_ctx,
        blocks,
    );
    if count_at > 0 {
        lower.tier_count = Some((Box::new(TCellU32::new(0)), count_at));
    }
    // only a body of pure ops gains: elsewhere every op that is not pure
    // commits everything anyway, and the exits store more
    lower.at_exits = at_exits && (0..pl.effective_end).all(|i| !syncs_before(pl.record, pl, i));
    let lw = &mut lower;
    emit_fold_precheck(lw, pl);
    emit_readonly_precheck(lw, pl);
    emit_step_precheck(lw, pl);
    emit_body(lw, pl)?;
    let (downrec_link_for_compiled, downrec_multi_way_count_for_compiled) = emit_tail(lw, pl)?;
    let Lower {
        bcx,
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        escape,
        global_side_trace_box,
        tier_count,
        ..
    } = lower;
    Some((
        bcx,
        Emitted {
            current_kinds,
            dispatchable,
            dispatch_off_reason,
            per_exit_kinds,
            per_exit_inline_vec,
            sunk_alloc_seen,
            materialize_emit_count,
            closure_seen,
            escape,
            global_side_trace_box,
            downrec_link_for_compiled,
            downrec_multi_way_count_for_compiled,
            tier_count,
        },
    ))
}
