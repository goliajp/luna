//! Tearing the state down.

use super::*;

impl Drop for Vm {
    fn drop(&mut self) {
        self.leave_host_entry();
        // state close: run `__gc` for every still-registered finalizable before
        // the heap frees them (PUC separatetobefnz(g,1) + callallpending). A
        // single pass — objects created by a closing finalizer are not
        // re-finalized (they go to the heap's free list directly).
        self.run_finalizers_closing();
        // a stream left open writes out its buffers before its memory goes
        crate::vm::lib_io::flush_all_closing(self);
        // the open upvalues the VM lists (5.1 and 5.2 keep them off the
        // heap's list) go there, for the heap to free
        let main = self.main_ctx.as_ref().map(|m| &m.open_upvals);
        for &(_, uv) in self.open_upvals.iter().chain(main.into_iter().flatten()) {
            // SAFETY: a listed upvalue is allocated, and nothing borrows
            // it; closed, it no longer reads the stack that goes too
            unsafe { uv.as_mut() }.set_closed(Value::Nil);
            self.heap.upval_closed(uv);
        }
        let id = self.jit_owner_id;
        // SAFETY: the finalizers were the last Lua code this Vm runs, and
        // its functions (the only holders of entry points compiled for it)
        // go away with its heap.
        unsafe {
            self.jit.storage.release_code(id);
            for s in &mut self.retired_jit_storage {
                s.release_code(id);
            }
        }
    }
}
