//! The string buffer a trace builds an accumulated concatenation in.

use super::*;

impl Vm {
    /// Pop a reusable `Vec<u8>` from the JIT accumulator buffer
    /// pool, or allocate one when the pool is empty. The trace keeps
    /// it (as the boxed pointer the helper leaks) in a stack slot
    /// through the loop and appends each piece to it.
    #[doc(hidden)]
    pub fn jit_str_buf_acquire(&mut self) -> Box<Vec<u8>> {
        Box::new(self.jit.str_buf_pool.pop().unwrap_or_default())
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend Vec.
    #[doc(hidden)]
    #[allow(clippy::boxed_local)] // the trace held the buffer boxed; it comes back that way
    pub fn jit_str_buf_release(&mut self, mut buf: Box<Vec<u8>>) {
        buf.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(*buf);
        }
        // Else: drop the buffer.
    }

    /// Append a piece's bytes to an accumulator buffer.
    #[doc(hidden)]
    pub fn jit_str_buf_extend(&mut self, buf: &mut Vec<u8>, piece: Gc<crate::runtime::LuaStr>) {
        buf.extend_from_slice(piece.as_bytes());
    }

    /// Drain the accumulator buffer into a fresh
    /// `LuaStr` via `heap.intern`, returning the raw ptr bits for
    /// the trace to write into the accumulator slot.
    ///
    /// Returns the LuaStr ptr as i64 on success, 0 on overflow
    /// (the hard cap; the trace deopts). The buffer is left
    /// CLEAR (drained) ready for release.
    #[doc(hidden)]
    pub fn jit_str_buf_intern(&mut self, buf: &mut Vec<u8>) -> i64 {
        let bytes = std::mem::take(buf);
        // hard cap at 256KB
        if bytes.len() > 256 * 1024 {
            return 0;
        }
        let gc = self.heap.intern(&bytes);
        gc.as_ptr() as i64
    }
}
