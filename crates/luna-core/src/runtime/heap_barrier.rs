//! Write barriers.

use super::*;

impl Heap {
    /// Forward write barrier: when a BLACK `parent` acquires a fresh reference
    /// to a WHITE `child`, gray the child (strings go straight to BLACK as
    /// leaves) and push onto the persistent gray queue so the next propagate
    /// step traces it. Mirrors PUC `luaC_barrier_`. No-op outside Propagate
    /// (parent is gray or white — the mutator never sees a BLACK object live
    /// outside an incremental cycle).
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Internal GC barrier; caller (Gc<T>::write_*) guarantees ptr validity per SAFETY below.
    pub fn barrier_forward(&mut self, parent: *mut GcHeader, child: Value) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            if !is_black((*parent).flags) {
                return;
            }
            let ch = match child {
                Value::Str(s) => s.as_ptr() as *mut GcHeader,
                Value::Table(t) => t.as_ptr() as *mut GcHeader,
                Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                Value::Native(n) => n.as_ptr() as *mut GcHeader,
                Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                _ => return,
            };
            let cf = (*ch).flags;
            if !is_white(cf) {
                return;
            }
            if (*ch).tag == ObjTag::Str {
                (*ch).flags = (cf & !COLOR_BITS) | BLACK;
            } else {
                (*ch).flags = cf & !WHITE_BITS;
                self.gray.push(ch);
            }
        }
    }

    /// Backward write barrier for objects with many fields (tables, threads):
    /// demote the parent itself back to gray so propagate re-traces it.
    /// Mirrors PUC `luaC_barrierback_`. One call covers any number of
    /// subsequent stores until the next propagate finishes — much cheaper for
    /// tables than per-child forward barriers. No-op outside Propagate.
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Internal GC barrier; caller (Gc<T>::write_*) guarantees ptr validity per SAFETY below.
    pub fn barrier_back(&mut self, parent: *mut GcHeader) {
        // SAFETY: `h` is a GcHeader pointer drawn from the runtime's all-objects intrusive list (or from a live `Gc<T>` cast above); it is non-null and remains live for the duration of this GC step (heap.rs:5-7).
        unsafe {
            let f = (*parent).flags;
            if !is_black(f) {
                return;
            }
            (*parent).flags = f & !COLOR_BITS;
            self.gray.push(parent);
        }
    }
}
