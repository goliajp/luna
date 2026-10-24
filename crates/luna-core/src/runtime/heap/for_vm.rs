//! What the heap keeps for the VM: values watched through a collection
//! without being kept alive, the chunks whose functions traces inline, the
//! memory cap, the standard input's handle.

use super::*;

/// Natives being fixed (see [`Heap::fix_natives`]).
#[must_use = "natives stay fixed until the token goes back to `Heap::stop_fixing_natives`"]
pub(crate) struct FixingNatives(());

impl Heap {
    /// Before a collection's atomic phase: `vals` watched, each found
    /// alive until the marking finds it dead.
    pub(crate) fn watch_values(&mut self, vals: impl IntoIterator<Item = Value>) {
        self.weak_watch.clear();
        self.weak_watch.extend(vals.into_iter().map(|v| (v, false)));
    }

    /// After it: the values [`Heap::watch_values`] watched, each with
    /// whether the collection found it dead.
    pub(crate) fn take_watched_values(&mut self) -> Vec<(Value, bool)> {
        std::mem::take(&mut self.weak_watch)
    }

    /// [`Heap::watch_values`] for objects.
    pub(crate) fn watch_objects(&mut self, objs: impl IntoIterator<Item = *const GcHeader>) {
        self.weak_objs.clear();
        self.weak_objs.extend(objs.into_iter().map(|o| (o, false)));
    }

    /// [`Heap::take_watched_values`] for objects.
    pub(crate) fn take_watched_objects(&mut self) -> Vec<(*const GcHeader, bool)> {
        std::mem::take(&mut self.weak_objs)
    }

    /// From now on, the main function of each chunk loaded is noted.
    #[inline]
    pub(crate) fn track_chunk_roots(&mut self) {
        self.track_chunk_roots = true;
    }

    /// `proto`, a chunk's main function, noted when chunks are tracked.
    pub(crate) fn note_chunk_root(&mut self, proto: Gc<Proto>) {
        if self.track_chunk_roots {
            self.chunk_roots.push(proto);
        }
    }

    /// The chunks noted and not found dead since.
    #[inline]
    pub(crate) fn chunk_roots(&self) -> &[Gc<Proto>] {
        &self.chunk_roots
    }

    /// The memory cap, if any.
    #[inline]
    pub(crate) fn mem_cap(&self) -> Option<usize> {
        self.mem_cap
    }

    #[inline]
    pub(crate) fn set_mem_cap(&mut self, cap: Option<usize>) {
        self.mem_cap = cap;
    }

    /// A raw store the fast path tried could not grow the table (`on`).
    #[inline]
    pub(crate) fn set_store_failed(&mut self, on: bool) {
        self.store_failed = on;
    }

    /// Whether [`Heap::set_store_failed`] said so; reset.
    #[inline]
    pub(crate) fn take_store_failed(&mut self) -> bool {
        std::mem::take(&mut self.store_failed)
    }

    /// The natives without upvalues made from now go on `fixed` (PUC's
    /// light C functions), until the token goes back to
    /// [`Heap::stop_fixing_natives`].
    #[inline]
    pub(crate) fn fix_natives(&mut self) -> FixingNatives {
        self.fix_natives = true;
        FixingNatives(())
    }

    #[inline]
    pub(crate) fn stop_fixing_natives(&mut self, _: FixingNatives) {
        self.fix_natives = false;
    }

    /// The threads the collector visits, taken while the VM shrinks their
    /// stacks, to hand back with [`Heap::put_back_threads`].
    #[inline]
    pub(crate) fn take_threads(&mut self) -> Vec<Gc<crate::runtime::Coro>> {
        std::mem::take(&mut self.threads)
    }

    #[inline]
    pub(crate) fn put_back_threads(&mut self, t: Vec<Gc<crate::runtime::Coro>>) {
        self.threads = t;
    }

    /// 5.1: the name `__gc`.
    #[inline]
    pub(crate) fn set_gc_event_51(&mut self, s: Gc<LuaStr>) {
        self.gc_event_51 = Some(s);
    }

    /// `io.stdin` as made.
    #[inline]
    pub(crate) fn set_stdin_ud(&mut self, h: *mut GcHeader) {
        self.stdin_ud = h;
    }

    #[inline]
    pub(crate) fn stdin_ud(&self) -> *mut GcHeader {
        self.stdin_ud
    }
}
