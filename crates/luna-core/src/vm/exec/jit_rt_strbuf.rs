//! The string buffer a trace builds an accumulated concatenation in.

use super::*;

impl Vm {
    /// Pop a reusable buffer from the JIT accumulator buffer pool, or
    /// make an empty one when the pool is empty. The trace keeps it (as
    /// the boxed pointer the helper leaks) in a stack slot through the
    /// loop and appends each piece to it.
    #[doc(hidden)]
    pub fn jit_str_buf_acquire(&mut self) -> Box<crate::runtime::mem::LVec<u8>> {
        let buf = self.jit.str_buf_pool.pop();
        Box::new(buf.unwrap_or_else(|| crate::runtime::mem::LVec::new(self.heap.mem())))
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend buffer.
    #[doc(hidden)]
    #[allow(clippy::boxed_local)] // the trace held the buffer boxed; it comes back that way
    pub fn jit_str_buf_release(&mut self, mut buf: Box<crate::runtime::mem::LVec<u8>>) {
        buf.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(buf.take());
        }
    }

    /// Append a piece's bytes to an accumulator buffer; `false`, the
    /// buffer as it was, when it cannot grow.
    #[doc(hidden)]
    pub fn jit_str_buf_extend(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
    ) -> bool {
        buf.extend_from_slice(piece.as_bytes()).is_ok()
    }

    /// Drain the accumulator buffer into a fresh string, returning its raw
    /// pointer bits for the trace to write into the accumulator slot. The
    /// buffer is left empty, its capacity kept, ready for release.
    ///
    /// Returns 0 when the string is past the hard cap or cannot be
    /// allocated; a deopt is parked then, so the dispatcher drops the
    /// trace's registers (the 0 among them) and the interpreter runs the
    /// loop again.
    #[doc(hidden)]
    pub fn jit_str_buf_intern(&mut self, buf: &mut crate::runtime::mem::LVec<u8>) -> i64 {
        // hard cap at 256KB
        let r = if buf.len() > 256 * 1024 {
            None
        } else {
            self.heap.try_intern(buf).ok()
        };
        buf.clear();
        match r {
            Some(gc) => gc.as_ptr() as i64,
            None => {
                self.jit_park_oom();
                0
            }
        }
    }
}
