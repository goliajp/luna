use super::*;

/// forward sweep over `record.ops[..effective_end]` to
/// classify NewTable sites. Conservative — over-escape is OK; the
/// rules below are correctness-preserving for any future emit (a
/// Sinkable site can always be heap-allocated; an Escaped site
/// MUST be).
///
/// Sweep rules:
/// - `Op::NewTable A=a B=cap`: always a new `Sinkable` site bound at
///   `(depth, a)` with `array_cap = B` (0 for `B == 0`, whose site
///   can only sink hash writes through `SetField`).
/// - `Op::SetList A=a`: the count is B, or the recorded `var_count`
///   when `B == 0`. Through a bound site with `C == 0`, no `k` and
///   `array_cap` equal to that count → sunk array init; otherwise the
///   site escapes. Bound source slots `A+1..=A+count` escape (nested
///   sinks are not handled).
/// - `Op::SetI A B_imm C`: a bound value slot C escapes. Through a
///   bound target, a key in `1..=array_cap` → sunk write; any other
///   key escapes the target.
/// - `Op::SetTable`: as `SetI`, with the key folded from a `LoadI`
///   (through `Move`s) by `const_fold_int_key`; no folded key → the
///   target escapes.
/// - `Op::SetField`: a bound value slot escapes; through a bound
///   target the constant key gets a hash slot → sunk write.
/// - `Op::GetI`: bound B with a key in `1..=array_cap` → sunk read,
///   else B escapes. `Op::GetField`: bound B with a key some earlier
///   `SetField` gave a slot → sunk read, else B escapes.
///   `Op::GetTable` / `Op::Len`: bound B (and GetTable's key C)
///   escapes. All unbind A.
/// - `Op::Move A=dst B=src`: A becomes an alias of src's site (or
///   unbound); src stays bound and does not escape. Exits materialise
///   a site into every register bound to it.
/// - `Op::Call A=fn B=narg+1`: bound argument slots `A..A+B-1` (up to
///   the top for `B = 0`) escape; the result registers are unbound.
/// - `Op::Return1 A=a`: a bound A escapes.
/// - Cmp ops (`Op::Lt/Le/Eq/EqK`): bound operands escape (a table is
///   compared by address); other live sites stay sunk and the side
///   exit materializes them.
/// - `Op::LoadNil`: unbinds `A..=A+B`. `Op::Close`: every live
///   binding escapes.
/// - Every other op (arithmetic, Concat, Closure captures, SetUpval,
///   SetTabUp, Test, TestSet, Not, Self, TForCall, Return, ...): a
///   bound register it reads escapes, since a sunk table's register
///   holds stale bits; the registers it writes are unbound.
///
/// Terminator handling (the op at `effective_end`, if any):
/// - `TraceEnd::Call`: every live binding escapes (the interpreter
///   resumes at the call with the whole frame live).
/// - `TraceEnd::ForLoop`: bindings below `A + 4` escape (the enclosing
///   scope's locals and the loop's control registers, live after the
///   loop and in the next iteration); the body's own locals stay sunk.
/// - `TraceEnd::InlineAbort` / `SelfLink` / `DownRec`: every live
///   binding escapes.
/// - `TraceEnd::Return`: the returned registers escape.
/// - No terminator (the trace runs back to its head): every live
///   binding escapes.
pub(super) fn escape_analyze(
    record: &TraceRecord,
    effective_end: usize,
    end_kind: Option<TraceEnd>,
    head_proto: Gc<Proto>,
    frame_w: usize,
) -> EscapeAnalysis {
    let max_stack = frame_w;
    // one bindings row per inline depth the record reaches
    let max_depth = record
        .ops
        .iter()
        .map(|r| r.inline_depth as usize)
        .max()
        .unwrap_or(0)
        .min(MAX_INLINE_DEPTH as usize)
        + 1;
    if max_stack == 0 {
        let upper0 = effective_end.min(record.ops.len());
        return EscapeAnalysis {
            sites: Vec::new(),
            op_actions: vec![None; upper0],
            live_at_op: vec![Vec::new(); upper0],
            accum_sites: Vec::new(),
            accum_live_at_op: Vec::new(),
        };
    }

    let mut bindings: Vec<Vec<Option<usize>>> = vec![vec![None; max_stack]; max_depth];
    let mut sites: Vec<AllocSite> = Vec::new();
    let upper = effective_end.min(record.ops.len());
    let mut op_actions: Vec<Option<OpAction>> = vec![None; upper];
    let mut live_at_op: Vec<Vec<LiveBinding>> = vec![Vec::new(); upper];

    for i in 0..upper {
        let cur_depth = record.ops[i].inline_depth as usize;
        // clear stale bindings from popped deeper
        // inline frames. After a Return*, control transitions
        // from depth N+1 back to depth N (the next op is at depth
        // N). The bindings rows for depths > N hold sites that
        // were tracked inside the now-popped frame; their
        // registers no longer point to live data. Without this
        // clear, live_at_op snapshots would include stale sites
        // and emit_materialize would index wrong inline windows.
        // (nothing is bound before the first site)
        if !sites.is_empty() {
            for d in (cur_depth + 1)..bindings.len() {
                for slot in bindings[d].iter_mut() {
                    *slot = None;
                }
            }
            // snapshot live sunk bindings BEFORE the op
            // processes (each cmp emit uses live_at_op[cmp_idx] to
            // materialise the right virt slots).
            let mut live_snap: Vec<LiveBinding> = Vec::new();
            for row in bindings.iter() {
                for (reg, &slot) in row.iter().enumerate() {
                    if let Some(sid) = slot {
                        live_snap.push(LiveBinding {
                            site: sid as u32,
                            reg: reg as u32,
                        });
                    }
                }
            }
            live_at_op[i] = live_snap;
        }

        let rop = &record.ops[i];
        let depth = rop.inline_depth;
        let ins = rop.inst;
        let a = ins.a();
        let op = ins.op();
        if (depth as usize) >= max_depth {
            // The lowerer bails on OOB; skip silently here so the
            // sweep stays a side-effect-free analysis.
            continue;
        }
        // another function's op neither makes nor reads a site (whose keys are
        // head constants); a store into an upvalue table lets its values escape
        if !std::ptr::eq(rop.proto.as_ptr(), head_proto.as_ptr()) || (a as usize) >= max_stack {
            let (reads, writes) = super::slots::rw_ranges(ins);
            for &(lo, n) in &reads {
                for r in lo..lo + n {
                    if (r as usize) < max_stack
                        && let Some(sid) = lookup(&bindings, depth, r)
                    {
                        mark_escape(&mut sites, sid);
                    }
                }
            }
            for &(lo, n) in &writes {
                for r in lo..lo + n {
                    if (r as usize) < max_stack {
                        unbind(&mut bindings, depth, r);
                    }
                }
            }
            continue;
        }
        sweep_op(
            record,
            i,
            rop,
            depth,
            ins,
            a,
            op,
            max_stack,
            &mut bindings,
            &mut sites,
            &mut op_actions,
        );
    }

    escape_at_end(
        record,
        effective_end,
        end_kind,
        max_stack,
        max_depth,
        &bindings,
        &mut sites,
    );

    let accum_sites = detect_accumulators(record, effective_end, head_proto);
    let accum_live_at_op = vec![Vec::new(); op_actions.len()];
    EscapeAnalysis {
        sites,
        op_actions,
        live_at_op,
        accum_sites,
        accum_live_at_op,
    }
}
