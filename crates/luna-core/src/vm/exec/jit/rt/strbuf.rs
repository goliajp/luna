//! The string buffer a trace builds an accumulated concatenation in.

use super::*;

/// What the string of a trace's accumulator buffer stands for (see
/// `TraceGhosts::accum`).
#[derive(Clone, Copy)]
pub(crate) enum AccumLast {
    /// the string the trace entered with, unchanged since
    Entry(Gc<crate::runtime::LuaStr>),
    /// PUC's long string, recorded
    Long(crate::runtime::puc::api::GhostRef),
}

impl Vm {
    /// Pop a reusable buffer from the JIT accumulator buffer pool, or
    /// make an empty one when the pool is empty. The trace keeps it (as
    /// the boxed pointer the helper leaks) in a stack slot through the
    /// loop and appends each piece to it.
    pub(crate) fn jit_str_buf_acquire(&mut self) -> Box<crate::runtime::mem::LVec<u8>> {
        let buf = self.jit.str_buf_pool.pop();
        let g = self.jit_ghosts();
        g.accum = None;
        g.accum_prefix = None;
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

    /// Start an accumulator buffer with the string the trace entered with.
    /// When the buffer cannot hold it, the string is kept aside: the trace
    /// goes on appending, and the exit leaves the string to the dispatcher
    /// (see [`Vm::jit_str_buf_intern`]). `false` then.
    pub(crate) fn jit_str_buf_extend(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
    ) -> bool {
        let g = self.jit_ghosts();
        g.accum = Some(AccumLast::Entry(piece));
        if buf.extend_from_slice(piece.as_bytes()).is_ok() {
            return true;
        }
        g.accum_prefix = Some(piece);
        false
    }

    /// The `CONCAT` at `op_pc` of a trace's accumulated concatenation:
    /// `piece` appended to the buffer, and the string PUC makes of the two
    /// counted (a short one made for real, so that it is found as PUC's
    /// is; a long one recorded, which the buffer's string stands for once
    /// made). -1, the buffer as it was, when the string cannot be made;
    /// 1 when a collection step falls due, the trace then leaving after
    /// the op.
    pub(crate) fn jit_str_buf_concat(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        piece: Gc<crate::runtime::LuaStr>,
        op_pc: u32,
    ) -> i64 {
        let had = buf.len();
        if buf.extend_from_slice(piece.as_bytes()).is_err() {
            return -1;
        }
        self.jit_ghosts().accum = None;
        if buf.len() <= crate::runtime::string::MAX_SHORT_LEN || self.version == LuaVersion::Lua51 {
            if self.heap.try_intern(buf).is_err() {
                buf.truncate(had);
                return -1;
            }
        } else {
            let g = self
                .heap
                .ghost_obj_ref(crate::runtime::puc::api::PucObj::Str {
                    len: buf.len(),
                    long: true,
                });
            self.jit_ghosts().accum = g.ok().map(AccumLast::Long);
        }
        if self.heap.gc_due() {
            self.jit.gc_exit = Some(op_pc);
            return 1;
        }
        0
    }

    /// Drain the accumulator buffer into a fresh string, returning its raw
    /// pointer bits for the trace to write into the accumulator slot
    /// `slot` of its window. The buffer is left empty, its capacity kept,
    /// ready for release.
    ///
    /// Returns 0 when the string is past the hard cap, cannot be made, or
    /// lacks the bytes it started with: the bytes move to `accum_unmade`
    /// (no allocation: the buffer's block moves) and the dispatcher makes
    /// the string outside compiled code, where the interpreter's
    /// allocation rules apply, before the 0 reaches anything else.
    pub(crate) fn jit_str_buf_intern(
        &mut self,
        buf: &mut crate::runtime::mem::LVec<u8>,
        slot: u32,
    ) -> i64 {
        let g = self.jit_ghosts();
        // the string PUC made last is this one
        let last = g.accum.take();
        let prefix = g.accum_prefix.take();
        if prefix.is_none()
            && let Some(AccumLast::Entry(e)) = last
            && e.as_bytes() == &buf[..]
        {
            buf.clear();
            return e.as_ptr() as i64;
        }
        // hard cap at 256KB
        let r = if prefix.is_some() || buf.len() > 256 * 1024 {
            None
        } else {
            self.accum_make(None, buf, last).ok()
        };
        if let Some(gc) = r {
            buf.clear();
            return gc.as_ptr() as i64;
        }
        self.unmade.accum = Some(AccumUnmade {
            prefix,
            bytes: buf.take(),
            slot,
            last,
        });
        self.unmade.any = true;
        0
    }

    /// The string of `prefix`'s bytes and then `bytes`, standing for PUC's
    /// long string when `last` records one.
    fn accum_make(
        &mut self,
        prefix: Option<Gc<crate::runtime::LuaStr>>,
        bytes: &[u8],
        last: Option<AccumLast>,
    ) -> Result<Gc<crate::runtime::LuaStr>, crate::runtime::mem::Oom> {
        let made = match last {
            Some(AccumLast::Long(g)) => Some(g),
            _ => None,
        };
        let off = made.map(|_| self.heap.counting_off());
        let r = match prefix {
            Some(p) => self.heap.try_intern_pair(p.as_bytes(), bytes),
            None => self.heap.try_intern(bytes),
        };
        if let Some(off) = off {
            self.heap.counting_on(off);
        }
        if let (Some(g), Ok(s)) = (made, &r) {
            self.heap.ghost_made(g, s.hdr());
        }
        r
    }

    /// After a trace's exit was restored to frame `base`, with the trace's
    /// window from register `off`: the register of the accumulated string
    /// the exit could not make, which now holds the string the trace
    /// entered with or nil, not the 0 the exit wrote (a collection may run
    /// before the string is made).
    pub(in crate::vm::exec) fn trace_hide_unmade_accum(
        &mut self,
        base: usize,
        off: usize,
    ) -> usize {
        let a = self.unmade.accum.as_ref().expect("an unmade string");
        let at = base + off + a.slot as usize;
        let v = a.prefix.map_or(Value::Nil, Value::Str);
        if let Some(s) = self.stack.get_mut(at) {
            *s = v;
        }
        at
    }

    /// The accumulated string the exit could not make, made now into the
    /// register `at` as the interpreter makes a string (an emergency
    /// collection and one more try, then the memory error, raised where
    /// the interpreter goes on). `drop`: an error is raised already, and
    /// the register stays as [`Self::trace_hide_unmade_accum`] left it.
    pub(in crate::vm::exec) fn trace_make_unmade_accum(&mut self, at: usize, drop: bool) {
        let Some(AccumUnmade {
            prefix,
            mut bytes,
            mut last,
            ..
        }) = self.unmade.accum.take()
        else {
            return;
        };
        // PUC's record is taken once: a collection before a retry may free it
        if !drop {
            self.unmade.made.accum += 1;
            match self.retry_oom(|vm| vm.accum_make(prefix, &bytes, last.take())) {
                Ok(s) => self.stack[at] = Value::Str(s),
                Err(e) => self.jit.pending_raise = Some(e),
            }
        }
        bytes.clear();
        if self.jit.str_buf_pool.len() < self.jit.str_buf_pool_cap {
            self.jit.str_buf_pool.push(bytes);
        }
    }
}

/// An accumulated string a trace's exit left unmade (see
/// [`Vm::jit_str_buf_intern`]).
pub(crate) struct AccumUnmade {
    prefix: Option<Gc<crate::runtime::LuaStr>>,
    bytes: crate::runtime::mem::LVec<u8>,
    /// the accumulator's register in the trace's window
    slot: u32,
    last: Option<AccumLast>,
}
