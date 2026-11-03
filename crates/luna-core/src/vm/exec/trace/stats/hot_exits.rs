//! The exits of compiled traces hot enough to start a side trace.

use super::*;
use crate::jit::trace::{CompiledTrace, ExitTag};

/// The resume pc and register tags of exit `idx` of `ct`, in the layout of
/// `exit_hit_counts`.
fn exit_at(ct: &CompiledTrace, idx: usize) -> (u32, TArc<[ExitTag]>) {
    let inline_n = ct.per_exit_inline.len();
    if idx < inline_n {
        let ent = &ct.per_exit_inline[idx];
        (ent.cont_pc, ent.exit_tags.clone())
    } else if idx < inline_n + ct.per_exit_tags.len() {
        let i = idx - inline_n;
        (
            ct.per_exit_tags.cont_pc(i),
            TArc::from(ct.per_exit_tags.tags(i)),
        )
    } else {
        (ct.head_pc, ct.exit_tags.clone())
    }
}

impl Vm {
    /// Surface every side-exit slot whose hit count is
    /// `>= HOTEXIT_THRESHOLD` across every trace reachable from
    /// `cl.proto` (recursively walking `proto.protos`). Returned
    /// entries are side-trace candidates: each carries the parent
    /// trace's `(head_proto, head_pc)`, the exit's index in the
    /// parent's `exit_hit_counts`, and the side trace's natural
    /// entry shape (`cont_pc` + `exit_tags`).
    ///
    /// Layout of `exit_hit_counts` (mirrored by the iter):
    /// - `[0..per_exit_inline.len())` → `InlineSideExit` (cont_pc +
    ///   window-sized exit_tags).
    /// - `[per_exit_inline.len()..inline.len() + per_exit_tags.len())`
    ///   → `per_exit_tags.tags(i)` (per-cont_pc caller-window tags).
    /// - Last slot → global clean-tail (cont_pc = `head_pc`,
    ///   exit_tags = `ct.exit_tags`).
    pub fn hot_exit_iter(
        &self,
        cl: crate::runtime::heap::Gc<crate::runtime::function::LuaClosure>,
    ) -> Vec<crate::jit::trace::HotExitInfo> {
        use crate::jit::trace::{HOTEXIT_THRESHOLD, HotExitInfo};
        fn walk(
            proto: crate::runtime::heap::Gc<crate::runtime::function::Proto>,
            out: &mut Vec<HotExitInfo>,
        ) {
            for ct in proto.traces.borrow().iter() {
                let inline_n = ct.per_exit_inline.len();
                let tags_n = ct.per_exit_tags.len();
                debug_assert_eq!(
                    ct.exit_hit_counts.len(),
                    inline_n + tags_n + 1,
                    "exit_hit_counts layout invariant violated"
                );
                for (idx, cell) in ct.exit_hit_counts.iter().enumerate() {
                    let hits = cell.get();
                    if hits < HOTEXIT_THRESHOLD {
                        continue;
                    }
                    let (cont_pc, exit_tags) = exit_at(ct, idx);
                    out.push(HotExitInfo {
                        head_proto: proto,
                        head_pc: ct.head_pc,
                        exit_idx: idx,
                        hits,
                        cont_pc,
                        exit_tags,
                    });
                }
            }
            for inner in proto.protos.iter() {
                walk(*inner, out);
            }
        }
        let mut out: Vec<HotExitInfo> = Vec::new();
        walk(cl.proto, &mut out);
        out
    }
}
