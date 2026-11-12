//! A compiled trace's parts, read.

use super::super::tier_up::TierUp;
use super::super::*;
use super::plans::PassCount;

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
    n_ops: u32 = copy;
    dispatchable: bool = copy;
    window_size: u32 = copy;
    per_exit_inline: &[InlineSideExit] = borrow;
    exit_hit_counts: &[TCellU32] = borrow;
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
    /// The entry tags of [`CompiledTraceParts::plans`]: per head-frame
    /// register, the tag the code is specialised to, or `ENTRY_TAG_ANY`.
    #[inline]
    pub fn entry_tags(&self) -> &[u8] {
        &self.plans.entry_tags
    }

    /// The clean tail's exit tags of [`CompiledTraceParts::plans`].
    #[inline]
    pub fn exit_tags(&self) -> &[ExitTag] {
        &self.plans.exit_tags
    }

    /// How the clean tail's exit tags restore: all untouched, all
    /// integers, or mixed.
    #[inline]
    pub fn global_tag_res_kind(&self) -> TagResKind {
        self.plans.global_tag_res_kind
    }

    /// The typed side exits' tag table of [`CompiledTraceParts::plans`].
    #[inline]
    pub fn per_exit_tags(&self) -> &ExitTagTable {
        &self.plans.per_exit_tags
    }

    /// The inline side exits' resume pcs and tags of
    /// [`CompiledTraceParts::plans`], in the order of `per_exit_inline`.
    #[inline]
    pub fn inline_exit_tags(&self) -> &ExitTagTable {
        &self.plans.inline_tags
    }

    /// Where the code counts its passes, as [`CompiledTraceParts::plans`]
    /// gives it.
    #[inline]
    pub fn pass_count(&self) -> &PassCount {
        &self.plans.pass_count
    }

    /// The entry side traces this root holds, in the order the dispatcher
    /// tries them.
    pub fn entry_side_traces(&self) -> Vec<crate::jit::send_compat::TArc<CompiledTrace>> {
        self.entry_sides.list.borrow().clone()
    }

    /// Whether this is an entry side trace of a root.
    #[inline]
    pub fn is_entry_side(&self) -> bool {
        self.entry_sides.is_one.get()
    }

    /// As [`CompiledTraceParts::tier_up`].
    #[inline]
    pub fn tier_up(&self) -> Option<&TierUp> {
        self.tier_up.as_deref()
    }

    /// As [`CompiledTraceParts::calls_helpers`], for the code it runs now.
    #[inline]
    pub fn calls_helpers(&self) -> bool {
        self.calls_helpers.get()
    }
}
