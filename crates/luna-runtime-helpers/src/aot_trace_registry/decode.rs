//! The exit tags and side exits of an AOT trace, unpacked from its meta.

use luna_core::__private::InlineSideExit;
use luna_core::jit::aot_meta::{PerExitTagsEntry, unpack_exit_tag};
use luna_core::jit::trace_types::{ExitTag, ExitTagTable};

/// The tags of `packed`; `None` when a byte is no tag.
pub(super) fn unpack_tags(packed: &[u8]) -> Option<Vec<ExitTag>> {
    packed.iter().map(|&b| unpack_exit_tag(b)).collect()
}

/// Each typed side exit's continuation pc and tags; `None` when a byte is
/// no tag.
pub(super) fn per_exit_tags(entries: &[PerExitTagsEntry]) -> Option<ExitTagTable> {
    let exits = entries
        .iter()
        .map(|e| Some((e.cont_pc, unpack_tags(&e.tags_packed)?)))
        .collect::<Option<Vec<_>>>()?;
    Some(ExitTagTable::from_exits(
        exits.iter().map(|(pc, t)| (*pc, &t[..])),
    ))
}

/// Rebuild the v3 `per_exit_inline` side exits of one trace. `None`
/// (after printing the probe line) when a chain or a packed tag is
/// corrupt, in which case the whole trace skips install.
pub(super) fn per_exit_inline(
    entries: &[luna_core::jit::aot_meta::PerExitInlineEntry],
    head_pc: u32,
    probe_on: bool,
) -> Option<Vec<InlineSideExit>> {
    let mut per_exit_inline_decoded: Vec<InlineSideExit> = Vec::with_capacity(entries.len());
    let mut inline_ok = true;
    for ent in entries {
        let Some(chain_vec) = ent.rebuild_chain() else {
            inline_ok = false;
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_chain_invalid (cont_pc={})",
                    head_pc, ent.cont_pc
                );
            }
            break;
        };
        let mut tags: Vec<ExitTag> = Vec::with_capacity(ent.tags_packed.len());
        for raw in ent.tags_packed.iter().copied() {
            if let Some(t) = unpack_exit_tag(raw) {
                tags.push(t);
            } else {
                inline_ok = false;
                break;
            }
        }
        if !inline_ok {
            if probe_on {
                eprintln!(
                    "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_tag_invalid (cont_pc={})",
                    head_pc, ent.cont_pc
                );
            }
            break;
        }
        per_exit_inline_decoded.push(InlineSideExit {
            cont_pc: ent.cont_pc,
            head_resume_pc: ent.head_resume_pc,
            exit_tags: tags.into(),
            chain: chain_vec.into(),
            side_trace_ptr: Box::new(luna_core::jit::send_compat::TCellPtr::null()),
        });
    }
    if !inline_ok {
        return None;
    }
    Some(per_exit_inline_decoded)
}
