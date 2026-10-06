use super::*;

/// Declares the virtual registers of each sinkable table site, demoting
/// the sites that cannot be sunk.
pub(super) fn alloc_sunk_sites<E: Emit>(
    bcx: &mut E,
    pl: &Plan<'_>,
    escape: &mut EscapeAnalysis,
) -> (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32) {
    let Plan {
        record,
        end_idx_opt,
        ..
    } = *pl;
    // allocate virtual `Variable`s for each Sinkable
    // site that meets the sunk-emit criteria. Sites that don't
    // meet the criteria are demoted to Escaped right here so the
    // body emit's site-state check naturally falls through to the
    // existing heap-alloc helper path. Criteria:
    //   - `inline_depth == 0` (trace head's frame only — inline
    //     sinking requires extra plumbing for
    //     the materialize helper to address inlined windows)
    //   - `array_cap` in `1..=MAX_SUNK_CAP` (cap = 0 means the
    //     site didn't decode an array part; cap > MAX is a
    //     Cranelift Variable budget guard)
    //   - the site's slot is NOT the trace-terminator `Op::Return1`
    //     R[A] — sinking that case needs the materialize helper
    //     to repack the array into a heap `Gc<Table>` on the way
    //     out
    //   - the trace's body has NO cmp ops (`Lt`/`Le`/`Eq`/`EqK`) —
    //     a cmp emits a side-exit and the interp resume needs the
    //     heap table; the sweep escapes all live bindings on
    //     a cmp, but we ALSO need to bail on body cmps that fire
    //     AFTER the site dies (no live binding to escape, but the
    //     trace still has a back-edge candidate).
    //
    // Note: looping traces (`opts.internal_loop = true`) that have
    // any cmp in body are already excluded by the sweep escape
    // rule. A ForLoop terminator escapes the bindings it carries
    // (below the loop's `A + 4`). So we don't need an explicit
    // `internal_loop` check here.
    const MAX_SUNK_CAP: u32 = 8;
    let return_a_for_sunk_check: Option<u32> = match end_idx_opt {
        Some((idx, TraceEnd::Return)) if idx < record.ops.len() => {
            let term = &record.ops[idx];
            if matches!(term.inst.op(), Op::Return1) && term.inline_depth == 0 {
                Some(term.inst.a())
            } else {
                None
            }
        }
        _ => None,
    };
    // There is no inline-cmp gate: inline cmp
    // side-exits (per_exit_inline arm) call
    // `emit_materialize_live_sunk` to reconstruct live sunk sites
    // before the frame-mat helper pushes inline frames, so a
    // depth>0 cmp doesn't demote sites.
    let mut virt_vars: Vec<Option<Vec<Variable>>> = vec![None; escape.sites.len()];
    let mut virt_kinds: Vec<Option<Vec<RegKind>>> = vec![None; escape.sites.len()];
    let mut sunk_alloc_seen: u32 = 0;
    for (idx, site) in escape.sites.iter_mut().enumerate() {
        if site.state != EscapeState::Sinkable {
            continue;
        }
        // depth>0 sites are sunk-eligible. Materialise
        // (`emit_materialize_live_sunk`) handles BOTH depth=0 and
        // depth>0 sites at depth=0 cmp arm AND inline cmp
        // (per_exit_inline) arm, since inline cmp side-exits
        // reconstruct live sunk sites. `return_a` check only matters for depth=0
        // (TraceEnd::Return applies at the trace-head frame).
        // total virt slot count = array_cap + hash_keys.
        // - array-only site:    cap = array_cap,           hash = 0
        // - hash-only site:     cap = 0,                   hash = hash_keys.len()
        // - mixed array+hash:   cap = array_cap > 0,       hash > 0
        // - empty (no ops):     cap = 0,                   hash = 0 → demoted below
        let array_cap = site.array_cap as usize;
        let n_hash = site.hash_keys.len();
        let total_slots = array_cap + n_hash;
        if total_slots == 0
            || array_cap > MAX_SUNK_CAP as usize
            || (site.inline_depth == 0 && return_a_for_sunk_check == Some(site.a))
        {
            site.state = EscapeState::Escaped;
            continue;
        }
        // hash slot materialise is plumbed into
        // emit_materialize_live_sunk (extended helper signature
        // carries hash_keys + hash_raws + hash_kinds buffers), so no
        // has_any_cmp gate is needed. Hash sites survive cmp side-exits via
        // table.set(Value::Str(key), ...) at materialise time.
        let mut vars = Vec::with_capacity(total_slots);
        for _ in 0..total_slots {
            let v = bcx.declare_var(types::I64);
            let z = bcx.ins().iconst(types::I64, 0);
            bcx.def_var(v, z);
            vars.push(v);
        }
        virt_vars[idx] = Some(vars);
        virt_kinds[idx] = Some(vec![RegKind::Unset; total_slots]);
        sunk_alloc_seen += 1;
    }
    (virt_vars, virt_kinds, sunk_alloc_seen)
}
