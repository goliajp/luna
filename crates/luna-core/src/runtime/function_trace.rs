//! Marking what a prototype or a function refers to.

use super::{LuaClosure, NativeClosure, Proto};
use crate::runtime::heap::{GcHeader, Marker};

impl Proto {
    pub(crate) fn trace(&self, m: &mut Marker) {
        for &k in self.consts.iter() {
            m.value(k);
        }
        for &p in self.protos.iter() {
            m.header(p.as_ptr() as *mut GcHeader);
        }
        m.header(self.source.as_ptr() as *mut GcHeader);
        // PUC `traverseproto`: the closure cache is a *weak* reference — if
        // the cached LClosure is unmarked at sweep time, clear the slot
        // instead of marking it. Queue self for the post-mark cleanup pass
        // so a closure whose only remaining live reference is the cache
        // becomes collectable (gc.lua's `__gc` finalisers inside `do ... end`
        // blocks rely on this).
        if self.cache.get().is_some() {
            m.cached_protos.push(self as *const Proto as *mut Proto);
        }
    }
}

impl LuaClosure {
    // kept inline in the drain loop, which visits every closure
    #[inline(always)]
    pub(crate) fn trace(&self, m: &mut Marker) {
        m.header(self.proto.as_ptr() as *mut GcHeader);
        for &uv in self.upvals().iter() {
            m.header(uv.as_ptr() as *mut GcHeader);
        }
    }
}

impl NativeClosure {
    pub(crate) fn trace(&self, m: &mut Marker) {
        for &v in self.upvals.iter() {
            m.value(v);
        }
    }
}
