//! The objects a trace being recorded went through.

use super::*;
use crate::jit::trace::TraceRecord;
use crate::runtime::heap::GcHeader;

/// The prototypes and the string `rec` refers to, none of them kept alive
/// by it.
pub(super) fn recording_objects(rec: &TraceRecord) -> impl Iterator<Item = *const GcHeader> + '_ {
    let h = |p: Gc<crate::runtime::Proto>| p.as_ptr() as *const GcHeader;
    std::iter::once(rec.head_proto)
        .chain(rec.ops.iter().map(|o| o.proto))
        .chain(rec.side_trace_parent.map(|p| p.0))
        .chain(rec.retfs.iter().map(|r| r.proto))
        .chain(rec.downrec_close.as_ref().map(|d| d.target_proto))
        .map(h)
        .chain(rec.index_key.map(|s| s.as_ptr() as *const GcHeader))
}
