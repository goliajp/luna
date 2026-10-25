//! Marking what a prototype or a function refers to.

use super::{LuaClosure, NativeClosure, Proto};
use crate::runtime::heap::Marker;
use crate::runtime::puc::records::parse::PARSING;

impl Proto {
    pub(crate) fn trace(&self, m: &mut Marker) {
        if self.hdr.cls & PARSING != 0 && m.trace_parsing_proto(self) {
            return;
        }
        for &k in self.consts.iter() {
            m.value(k);
        }
        for &p in self.protos.iter() {
            m.mark(p);
        }
        m.mark(self.source);
        // a trace's code checks callees against the protos it inlined by
        // address: one freed and reallocated would pass that check
        for &p in self.inlined_protos.borrow().iter() {
            m.mark(p);
        }
        for &s in self.puc_names.borrow().iter() {
            m.mark(s);
        }
        // PUC `traverseproto`: the closure cache is a weak reference, let
        // go of when the closure is white as the prototype is traversed
        if let Some(c) = self.cache.get()
            && m.lets_go_of(c.hdr())
        {
            self.cache.set(None);
        }
    }
}

impl LuaClosure {
    // kept inline in the drain loop, which visits every closure
    #[inline(always)]
    pub(crate) fn trace(&self, m: &mut Marker) {
        if self.hdr.cls & PARSING != 0 && m.trace_parsing_closure(self) {
            return;
        }
        m.mark(self.proto);
        for &uv in self.upvals().iter() {
            m.upval(uv);
        }
    }
}

impl NativeClosure {
    /// Whether this native is luna's own, no function of PUC's (the host's
    /// protected-call levels).
    #[inline]
    pub(crate) fn is_luna_only(&self) -> bool {
        self.hdr.cls & crate::runtime::heap::LUNA_ONLY != 0
    }

    /// The upvalues PUC's function has; luna's others with luna's own
    /// objects.
    pub(crate) fn trace(&self, m: &mut Marker) {
        // 5.1: its environment first
        m.mark_env51(&self.hdr as *const _ as *mut _, self.hdr.cls);
        let puc = crate::runtime::puc::native_puc_upvals(self);
        for (i, &v) in self.upvals.iter().enumerate() {
            if i < puc {
                m.value(v);
            } else {
                m.quiet_value(v);
            }
        }
    }
}
