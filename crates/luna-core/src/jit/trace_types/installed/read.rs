//! A compiled trace's parts, read.

use super::super::tier_up::TierUp;
use super::super::*;

/// One method per field, returning it by value or by reference.
macro_rules! readers {
    ($($f:ident: $t:ty = $how:tt;)*) => {
        impl CompiledTrace {
            $(
                #[doc = concat!("As [`CompiledTraceParts::", stringify!($f), "`].")]
                #[inline]
                pub fn $f(&self) -> $t {
                    readers!(@ $how self.$f)
                }
            )*
        }
    };
    (@ copy $e:expr) => { $e };
    (@ borrow $e:expr) => { &$e };
}

readers! {
    head_pc: u32 = copy;
    entry: TraceFn = copy;
    n_ops: u32 = copy;
    dispatchable: bool = copy;
    window_size: u32 = copy;
    exit_tags: &TArc<[ExitTag]> = borrow;
    global_tag_res_kind: TagResKind = copy;
    entry_tags: &TArc<[u8]> = borrow;
    per_exit_tags: &ExitTagTable = borrow;
    per_exit_inline: &[InlineSideExit] = borrow;
    exit_hit_counts: &[TCellU32] = borrow;
    exit_side_trace_ptrs: &[TCellPtr] = borrow;
    tags_side_trace_ptrs: &[TCellPtr] = borrow;
    global_side_trace_ptr: &TCellPtr = borrow;
    is_inline_abort_close: bool = copy;
    dispatch_off_reason: Option<&'static str> = copy;
    sinkable_sites_seen: u32 = copy;
    accum_bufferable_seen: u32 = copy;
    sunk_alloc_seen: u32 = copy;
    materialize_emit_count: u32 = copy;
    closure_seen: u32 = copy;
    inline_kinds: u8 = copy;
    inline_depth: u8 = copy;
    body_writes: &[u32] = borrow;
    downrec_link: Option<(u32, u32)> = copy;
    downrec_multi_way_count: u8 = copy;
}

impl CompiledTrace {
    /// As [`CompiledTraceParts::tier_up`].
    #[inline]
    pub fn tier_up(&self) -> Option<&TierUp> {
        self.tier_up.as_deref()
    }
}
