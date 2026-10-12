//! Call-site context records kept by the Vm.

/// Call-site context an in-flight async native
/// needs preserved across the cooperative-yield boundary.
///
/// The dispatcher records this when it routes a `NativeClosure` with
/// `is_async == true` through the cooperative path; `EvalFuture::poll`
/// hands it back to [`Vm::commit_async_native_result`] once the
/// awaited future resolves so `finish_results` (and the post-call GC
/// checkpoint) can run as if the native had completed synchronously.
#[derive(Clone, Copy)]
pub(crate) struct AsyncNativeCallCtx {
    pub func_slot: u32,
    /// Recorded for parity with the sync native-call path's
    /// `native_nresults`/`gc_top` bookkeeping; reserved for hook
    /// firing + traceback shaping. Not read yet.
    #[allow(dead_code)]
    pub nargs: u32,
    pub nresults: i32,
    /// Recorded for traceback + GC-root-window checks. The resume path
    /// reads `Vm.gc_top` directly, so this is unread today; carried so a
    /// check can confirm the pre-suspend root window matches the
    /// post-resume one.
    #[allow(dead_code)]
    pub gc_top: u32,
}

/// The counters every call checks and keeps (see each field's note):
/// together so a call touches one cache line for all of them.
#[repr(C)]
pub(crate) struct CallGuards {
    /// PUC `nCcalls`: the C levels in flight on the running thread (see
    /// `MAX_C_DEPTH`): calls native code made into Lua, the pcall /
    /// xpcall, metamethod, `__pairs` and `__close` continuations above the
    /// thread's last resume, the resumes below it, and the levels refused
    /// calls hold while their message handlers run. A resume starts the
    /// coroutine from the resumer's count (`lua_resume`), so a thread's
    /// count is not saved with it.
    pub(crate) nccalls: u32,
    /// frames the running thread had when it was last resumed: PUC's
    /// `lua_resume` starts the thread from the resumer's count, so the
    /// continuations below hold no level and popping them gives none
    /// back. Per-thread, saved with the coroutine context.
    pub(crate) stale_frames: u32,
    /// the size of PUC 5.1's `CallInfo` array: doubled when the frames in
    /// use (`Vm::frames_in_use`) fill it, which is where its limit is
    /// checked (`grow_frames`). Per-thread. `u32::MAX` in later dialects.
    pub(crate) frame_size: u32,
    /// `lua_stack_limit` of the dialect
    pub(crate) lua_stack_limit: u32,
    /// calls compiled code made on the native stack below the running
    /// interpreter frames, which take no frame but count against 5.1's
    /// call limit
    pub(crate) frames_native: u32,
    /// frames the host holds below the main thread's (`host_entry_layout`)
    pub(crate) host_frames: u32,
    /// metamethod, `__pairs` and `__close` continuation frames on the
    /// running thread: no frame of PUC's (its metamethod call makes one
    /// frame, the callee's), so not counted in `frames_in_use`.
    /// Per-thread, saved with the coroutine context.
    pub(crate) meta_conts: u32,
}
