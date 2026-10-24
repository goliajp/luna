//! The string buffer a trace builds an accumulated concatenation in.

use super::*;

/// What the string of a trace's accumulator buffer stands for (see
/// `TraceGhosts::accum`).
#[derive(Clone, Copy)]
pub(crate) enum AccumLast {
    /// the string the trace entered with, unchanged since
    Entry(Gc<crate::runtime::LuaStr>),
    /// PUC's long string, recorded
    Long(crate::runtime::heap::GhostRef),
}

impl Vm {
    /// Pop a reusable buffer from the JIT accumulator buffer pool, or
    /// make an empty one when the pool is empty. The trace keeps it (as
    /// the boxed pointer the helper leaks) in a stack slot through the
    /// loop and appends each piece to it.
    pub(crate) fn jit_str_buf_acquire(&mut self) -> Box<crate::runtime::mem::LVec<u8>> {
        let buf = self.jit.str_buf_pool.pop();
        self.jit_ghosts().accum = None;
        Box::new(buf.unwrap_or_else(|| crate::runtime::mem::LVec::new(self.heap.mem())))
    }

    /// Return a previously-acquired buffer to the
    /// pool, dropping any excess past `jit_str_buf_pool_cap`. The
    /// buffer is `clear`ed (capacity retained) so the next acquire
    /// gets a ready-to-extend buffer.
    #[allow(clippy::boxed_local)] // the trace held the buffer boxed; it comes back that way
    pub(crate) fn jit_str_buf_release(&mut self, mut buf: Box<crate::runtime::mem::LVec<u8>>) {
        buf.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(buf.take());
        }
    }

    /// Append a piece's bytes to an accumulator buffer; `false`, the
    /// buffer as it was, when it cannot grow.
    pub(crate) fn jit_str_buf_extend(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
    ) -> bool {
        // the trace's first append: the string it entered with
        self.jit_ghosts().accum = Some(AccumLast::Entry(piece));
        buf.extend_from_slice(piece.as_bytes()).is_ok()
    }

    /// The `CONCAT` at `op_pc` of a trace's accumulated concatenation:
    /// `piece` appended to the buffer, and the string PUC makes of the two
    /// counted (a short one made for real, so that it is found as PUC's
    /// is; a long one recorded, which the buffer's string stands for once
    /// made). -1 when the buffer cannot grow; 1 when a collection step
    /// falls due, the trace then leaving after the op.
    pub(crate) fn jit_str_buf_concat(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
        op_pc: u32,
    ) -> i64 {
        if buf.extend_from_slice(piece.as_bytes()).is_err() {
            return -1;
        }
        self.jit_ghosts().accum = None;
        if buf.len() <= crate::runtime::string::MAX_SHORT_LEN || self.version == LuaVersion::Lua51 {
            if self.heap.try_intern(buf).is_err() {
                return -1;
            }
        } else {
            let size = self.heap.puc_size(crate::runtime::puc::PucObj::Str {
                len: buf.len(),
                long: true,
            });
            let g = self
                .heap
                .acct_ghost_ref(crate::runtime::heap::PucKind::LongStr, size);
            self.jit_ghosts().accum = g.ok().map(AccumLast::Long);
        }
        if self.heap.gc_due() {
            self.jit.gc_exit = Some(op_pc);
            return 1;
        }
        0
    }

    /// Drain the accumulator buffer into a fresh string, returning its raw
    /// pointer bits for the trace to write into the accumulator slot. The
    /// buffer is left empty, its capacity kept, ready for release.
    ///
    /// Returns 0 when the string is past the hard cap or cannot be
    /// allocated; a deopt is parked then, so the dispatcher drops the
    /// trace's registers (the 0 among them) and the interpreter runs the
    /// loop again.
    pub(crate) fn jit_str_buf_intern(&mut self, buf: &mut crate::runtime::mem::LVec<u8>) -> i64 {
        // hard cap at 256KB
        // the string PUC made last is this one
        let last = self.jit_ghosts().accum.take();
        if let Some(AccumLast::Entry(e)) = last
            && e.as_bytes() == &buf[..]
        {
            buf.clear();
            return e.as_ptr() as i64;
        }
        let made = match last {
            Some(AccumLast::Long(g)) => Some(g),
            _ => None,
        };
        let off = made.map(|_| self.heap.counting_off());
        let r = if buf.len() > 256 * 1024 {
            None
        } else {
            self.heap.try_intern(buf).ok()
        };
        if let Some(off) = off {
            self.heap.counting_on(off);
        }
        if let (Some(g), Some(s)) = (made, r) {
            self.heap.ghost_made(g, s.hdr());
        }
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
