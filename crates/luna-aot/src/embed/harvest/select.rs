//! Which recordings the warmup captured go into the AOT binary, each paired
//! with the trace it compiled into.

use luna_core::__private::ProtoTraceInternals;
use luna_core::__private::{CompiledTrace, TraceRecord};
use luna_core::jit::aot_meta::AotRole;

use super::Installable;

/// Pair each captured record with the `CompiledTrace` that landed in
/// its proto and keep the AOT-installable ones.
pub(super) fn select_installable(
    captured: Vec<([u8; 16], u32, TraceRecord)>,
    probe_on: bool,
) -> Vec<Installable> {
    // the trace's fields after lowering go in the meta blob (`pair`);
    // the wire format carries `per_exit_tags` (v2) and `per_exit_inline`
    // (v3), not sunk-alloc materialize sites, which stay JIT-only
    let mut installable: Vec<Installable> = Vec::new();
    let mut filter_stats = (0usize, 0usize, 0usize, 0usize);
    let mut roots: Vec<*const CompiledTrace> = Vec::new();
    let mut sides: Vec<Installable> = Vec::new();
    for (i, (hash, head_pc, record)) in captured.into_iter().enumerate() {
        let Some((ct, role)) = pair(&record) else {
            filter_stats.0 += 1;
            continue;
        };
        // two recordings of one trace (a recording compiled, then cut and
        // compiled again): the first
        let taken = |t: &Installable| luna_core::jit::send_compat::TArc::ptr_eq(&t.4, &ct);
        if installable.iter().chain(&sides).any(taken) {
            continue;
        }
        if !ct.dispatchable() {
            filter_stats.1 += 1;
            continue;
        }
        if !ct.per_exit_inline().is_empty() {
            // depth>0 inlined cmp side-exits are supported. The lowerer's
            // `emit_chain_ptr_arg` routes the `FrameMaterializeInfo`
            // chain pointer through a relocatable data slot the
            // deploy-side `aot_inline_chain_resolver` populates at
            // startup; the v3 wire format's `per_exit_inline` tail
            // (cont_pc / head_resume_pc / packed exit_tags / packed
            // chain bytes) round-trips into a fresh
            // `Rc<[InlineSideExit]>` on the install side. The JIT path
            // bakes that chain pointer as a raw `iconst`, which under
            // AOT would be the warmup VM's heap address, invalid in the
            // deploy binary. Stat counter for diagnostics: how many of
            // the captured traces went through the v3 inline path.
            filter_stats.2 += 1;
        }
        // per_exit_tags is supported by wire format v2 — accept.
        // (Counter for diagnostics: how many of the captured traces
        // went through the v2 path.)
        if !ct.per_exit_tags().is_empty() {
            filter_stats.3 += 1;
        }
        match role {
            AotRole::Root => {
                roots.push(luna_core::jit::send_compat::TArc::as_ptr(&ct));
                installable.push((i, hash, head_pc, record, ct, role));
            }
            AotRole::EntrySide => sides.push((i, hash, head_pc, record, ct, role)),
        }
    }
    installable.extend(after_roots(sides, &roots));

    if probe_on {
        eprintln!(
            "luna-aot harvest: installable={}, filtered: no_ct={}, undispatchable={}, accepted_with_per_exit_inline={}, accepted_with_per_exit_tags={}",
            installable.len(),
            filter_stats.0,
            filter_stats.1,
            filter_stats.2,
            filter_stats.3,
        );
    }
    installable
}

/// The entry side traces `sides` whose root is among `roots`, in the order
/// of their roots' lists: they go after every root, each only with its own.
fn after_roots(mut sides: Vec<Installable>, roots: &[*const CompiledTrace]) -> Vec<Installable> {
    let order = |t: &Installable| {
        let root = head_root(&t.3).map(|r| (r.entry_side_traces(), r));
        root.and_then(|(list, r)| {
            let at = list
                .iter()
                .position(|e| luna_core::jit::send_compat::TArc::ptr_eq(e, &t.4))?;
            Some((luna_core::jit::send_compat::TArc::as_ptr(&r), at))
        })
    };
    sides.retain(|t| order(t).is_some_and(|(r, _)| roots.contains(&r)));
    sides.sort_by_key(|t| order(t).map(|(_, at)| at));
    sides
}

/// The trace at the head of `record` that is no side trace.
fn head_root(record: &TraceRecord) -> Option<luna_core::jit::send_compat::TArc<CompiledTrace>> {
    let traces = record.head_proto.traces();
    traces
        .iter()
        .find(|t| t.head_pc() == record.head_pc && !t.is_side_trace())
        .cloned()
}

/// The trace `record` compiled into, by what it is rather than where it
/// starts: a side trace's recording pairs with none (side traces stay
/// JIT-only), a root's with the head's trace, an entry side trace's with
/// the one of its root's that takes its entry tags (at most one does).
fn pair(
    record: &TraceRecord,
) -> Option<(luna_core::jit::send_compat::TArc<CompiledTrace>, AotRole)> {
    if record.side_trace_parent.is_some() {
        return None;
    }
    let root = head_root(record)?;
    if !record.entry_side {
        return Some((root, AotRole::Root));
    }
    let es = (root.entry_side_traces().into_iter())
        .find(|e| luna_core::__private::entry_tags_admit(e.entry_tags(), &record.entry_tags))?;
    Some((es, AotRole::EntrySide))
}
