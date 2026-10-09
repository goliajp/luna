//! What escapes when the trace ends.

use super::*;

/// What the trace terminator (the op at `effective_end`) forces to escape.
pub(super) fn escape_at_end(
    record: &TraceRecord,
    effective_end: usize,
    end_kind: Option<TraceEnd>,
    max_stack: usize,
    max_depth: usize,
    bindings: &[Vec<Option<usize>>],
    sites: &mut [AllocSite],
) {
    if let Some(end) = end_kind
        && effective_end < record.ops.len()
    {
        let term = &record.ops[effective_end];
        let depth = term.inline_depth;
        let a = term.inst.a();
        let op = term.inst.op();
        let in_range = (depth as usize) < max_depth && (a as usize) < max_stack;
        match end {
            // The interpreter resumes at the call with the whole frame
            // live: a table still under construction (`{f()}`, whose
            // SetList follows the call) or any other sunk table read
            // after it must be a real one.
            TraceEnd::Call | TraceEnd::InlineAbort => {
                escape_all_live(bindings, sites);
            }
            TraceEnd::ForLoop => {
                // The terminator either leaves the loop, where the
                // registers below the loop's own (`A`) are the
                // enclosing scope's locals, or jumps back to the
                // body's first op, where the next iteration reads
                // them and the control registers `A..A+4`. A sunk
                // table never reaches either: its registers hold
                // stale bits, and the back-edge has no materialise
                // path. So a table still bound there (`last = t`,
                // `prev = {n = i}`) is a real one. Registers from
                // `A + 4` up are the body's locals, out of scope at
                // both places, and keep their tables sunk.
                let row = depth as usize;
                if row < bindings.len() {
                    let limit = (a as usize).saturating_add(4).min(bindings[row].len());
                    for &slot in &bindings[row][..limit] {
                        if let Some(sid) = slot {
                            mark_escape(sites, sid);
                        }
                    }
                }
            }
            TraceEnd::Return => {
                // the returned values: R[A] for Return1, R[A..A+B-1)
                // for Return (B = 0: up to the top)
                let end = match op {
                    Op::Return1 => a.saturating_add(1),
                    Op::Return if term.inst.b() == 0 => max_stack as u32,
                    Op::Return => a.saturating_add(term.inst.b().saturating_sub(1)),
                    _ => a,
                };
                if in_range {
                    for r in a..end.min(max_stack as u32) {
                        if let Some(sid) = lookup(bindings, depth, r) {
                            mark_escape(sites, sid);
                        }
                    }
                }
            }
            TraceEnd::SelfLink(_) => {
                // self-link close. Body loops with
                // snapshot-restore (deepest-frame's window →
                // head-frame's window). Every live binding at
                // close must be marshalled back across the
                // back-edge so the next iter sees a coherent
                // window, same as InlineAbort's blanket escape.
                escape_all_live(bindings, sites);
            }
            TraceEnd::DownRec { .. } => {
                // down-rec close. Every exit (the safe deopt
                // tail and the retf-guard exit alike) returns
                // through the caller window, so every live
                // binding at close must be marshalled back into
                // it — same blanket-escape posture as SelfLink /
                // InlineAbort.
                escape_all_live(bindings, sites);
            }
        }
    } else if end_kind.is_none() {
        // No terminator: the trace runs back to its head, either
        // through the dispatcher or along its own back-edge. Which
        // registers are still in scope there is not known (a while
        // loop's locals have no `A` to split them by), so every table
        // still bound is a real one.
        escape_all_live(bindings, sites);
    }
}
