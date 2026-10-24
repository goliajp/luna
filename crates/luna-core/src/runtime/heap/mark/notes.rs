//! What the traversals outside the heap read from the marker and tell it:
//! the dialect, the tables to clear once marking is done, the closure
//! caches, and what PUC's records mark through it.

use super::*;
use crate::runtime::puc::for_heap::ParseFrame;

impl Marker {
    /// The dialect whose marking rules apply.
    #[inline]
    pub(crate) fn dialect(&self) -> crate::runtime::dialect::Dialect {
        self.rules.dialect()
    }

    /// The model's rules for the dialect.
    #[inline(always)]
    pub(in crate::runtime) fn rules(&self) -> &crate::runtime::puc::for_heap::HeapRules {
        &self.rules
    }

    /// A marked table with a weak mode: its dead entries are cleared once
    /// marking is done.
    #[inline]
    pub(crate) fn note_weak(&mut self, t: *mut Table) {
        self.weak.push(t);
    }

    /// Whether weak-keyed tables are ephemerons, whose values are marked
    /// only through live keys (not in 5.1).
    #[inline]
    pub(crate) fn ephemerons(&self) -> bool {
        !self.no_ephemeron
    }

    /// A marked ephemeron table: its values are marked by the convergence
    /// once the reachable keys are known.
    #[inline]
    pub(crate) fn note_ephemeron(&mut self, t: *mut Table) {
        self.ephemeron.push(t);
    }

    /// A marked prototype with a cached closure, cleared once marking is
    /// done if nothing else marked the closure.
    #[inline]
    pub(crate) fn note_cached_proto(&mut self, p: *mut crate::runtime::Proto) {
        self.cached_protos.push(p);
    }

    /// The loads the marking runs in the middle of.
    pub(in crate::runtime) fn all_load_frames(&self) -> &[ParseFrame] {
        &self.lent.parse
    }

    /// [`Marker::all_load_frames`], none while marking what luna's own
    /// objects hold (which keeps them all).
    pub(in crate::runtime) fn load_frames(&self) -> &[ParseFrame] {
        if self.quiet_mode {
            return &[];
        }
        self.all_load_frames()
    }

    /// Mark what luna's own objects hold with nothing counted (`on`), or
    /// count again.
    #[inline]
    pub(in crate::runtime) fn set_quiet(&mut self, on: bool) {
        self.quiet_mode = on;
    }

    /// 5.5: what the marking counted, taken back by the model.
    #[inline]
    pub(in crate::runtime) fn take_sizer(&mut self) -> Option<super::super::collect::Sizer> {
        self.sizer.take()
    }
}
