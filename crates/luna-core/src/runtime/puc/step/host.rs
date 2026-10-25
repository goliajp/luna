//! What PUC's steps need of the VM, which holds the threads and their
//! stacks, marks from its roots and runs the finalizers.

use super::*;

/// What a step needs of the VM: the threads it holds and their stacks, the
/// marking from its roots, its finalizers.
pub(crate) trait StepHost {
    fn heap(&mut self) -> &mut Heap;
    /// The running thread and the main one, as the collector finds them.
    fn running(&self) -> Running;
    /// Before a step: what the VM keeps of objects a step may free.
    fn before_step(&mut self);
    /// PUC's propagation with `work` to do (down to `floor`); true when no
    /// object is gray.
    fn propagate(&mut self, work: &mut i64, floor: i64, run: Running) -> bool;
    /// The roots `restartcollection` marks.
    fn mark_roots_restart(&mut self, m: &mut Marker);
    /// Every root: the state's (`mark_state_roots`) and the threads the VM
    /// holds (`mark_threads_held`); `again` in an atomic step.
    fn mark_roots(&mut self, m: &mut Marker, again: bool);
    /// The roots of the state: the globals, the registry, the metatables.
    fn mark_state_roots(&mut self, m: &mut Marker, again: bool);
    /// The stacks of the threads the VM holds.
    fn mark_threads_held(&mut self, m: &mut Marker);
    /// The marking is about to end: the values the VM watches without
    /// keeping them, and its threads' open upvalues, lent to the heap.
    fn marking_ends(&mut self);
    /// The collection is done: the watched values it found dead forgotten.
    fn collection_ends(&mut self);
    /// The running stack's slots above the top are dead.
    fn clear_dead_stack(&mut self);
    /// The running thread and the main one shrink.
    fn shrink_stacks(&mut self);
    /// Each coroutine marked live that is not running shrinks.
    fn shrink_threads(&mut self);
    /// Whether a finalizer is running, which no collection interrupts.
    fn finalizing(&self) -> bool;
    /// Whether this cycle's propagation has shrunk the running thread.
    fn running_shrunk(&self) -> bool;
    /// An emergency collection's visit of every thread, which 5.2's sweep
    /// shrinks.
    fn shrink_threads_emergency(&mut self);
    /// The list sweep of `quota` objects: how many it swept and the bytes
    /// it freed; `None` once the list is done.
    fn sweep_quota(&mut self, quota: i64) -> Option<(i64, i64)>;
    /// `GCTM`: the next finalizer run.
    fn run_finalizer(&mut self, propagate: bool) -> Result<(), LuaError>;
    /// Whether the stack has room for a finalizer's call.
    fn finalizer_has_room(&mut self, closing: bool) -> bool;
    /// 5.2's `sweepthread` of coroutine `h`, which the sweep just kept
    /// (the main thread's comes at the end of the sweep).
    fn shrink_swept_thread_52(&mut self, co: crate::runtime::Gc<crate::runtime::Coro>);
    /// The end of 5.2's sweep: the main thread shrunk.
    fn sweep_main_52(&mut self);
}

/// Which of a thread's `CallInfo`s PUC's traversal counts.
#[derive(Clone, Copy)]
pub(crate) enum CiCount {
    /// 5.1's array
    Array51,
    /// 5.2: the levels below the running one
    InUse,
    /// all the thread has
    Allocated,
}

impl Heap {
    /// Which `CallInfo`s PUC's traversal of a thread counts.
    pub(crate) fn puc_ci_count(&self) -> CiCount {
        match self.puc.dialect {
            Dialect::L51 => CiCount::Array51,
            Dialect::L52 => CiCount::InUse,
            _ => CiCount::Allocated,
        }
    }
}
