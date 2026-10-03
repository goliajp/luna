//! Natives the call path runs itself instead of calling their function:
//! async natives, and the yieldable `pcall` / `xpcall` / `pairs`. Which
//! one a closure is was fixed when it was created ([`NativeKind`]).

use super::*;

/// See [`crate::runtime::NativeClosure::kind`].
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeKind {
    Plain,
    Async,
    Pcall,
    Xpcall,
    HostXpcall,
    Pairs,
}

impl NativeKind {
    /// The kind of a synchronous native calling `f`.
    pub(crate) fn of(f: crate::runtime::value::NativeFn) -> NativeKind {
        use crate::runtime::value::NativeFn;
        use crate::vm::builtins::{
            nat_host_xpcall, nat_host_xpcall_in_c, nat_pairs, nat_pcall, nat_xpcall,
        };
        if std::ptr::fn_addr_eq(f, nat_pcall as NativeFn) {
            NativeKind::Pcall
        } else if std::ptr::fn_addr_eq(f, nat_xpcall as NativeFn) {
            NativeKind::Xpcall
        } else if std::ptr::fn_addr_eq(f, nat_host_xpcall as NativeFn)
            || std::ptr::fn_addr_eq(f, nat_host_xpcall_in_c as NativeFn)
        {
            NativeKind::HostXpcall
        } else if std::ptr::fn_addr_eq(f, nat_pairs as NativeFn) {
            NativeKind::Pairs
        } else {
            NativeKind::Plain
        }
    }
}

impl Vm {
    /// `stack[func_slot]` is `nc`, a native whose kind is not
    /// [`NativeKind::Plain`]. `None` when it is to be called like any other
    /// native after all (`pairs` without a `__pairs` to honour).
    #[inline(never)]
    pub(super) fn begin_special_native(
        &mut self,
        nc: Gc<crate::runtime::NativeClosure>,
        func_slot: u32,
        nargs: u32,
        nresults: i32,
    ) -> Option<Result<bool, LuaError>> {
        match nc.kind {
            NativeKind::Async => {
                // Async-marked NativeClosure.
                // Route through the cooperative-yield mechanism
                // when async_mode is on; reject when called from
                // a sync `eval`/`call_value` path (would have no
                // executor to drive the returned future).
                if !self.async_mode {
                    let s = Value::Str(self.heap.intern(b"async native called in sync context"));
                    self.last_error_kind = crate::vm::error::LuaErrorKind::Runtime;
                    return Some(Err(LuaError(s)));
                }
                // Same root-up bookkeeping as the sync path:
                // pin args + result-count expectation so a
                // collection across the suspend boundary
                // keeps the arg window live.
                self.native_nresults = nresults;
                self.gc_top = func_slot + nargs + 1;
                // Fire the "call" hook BEFORE
                // building the future. Mirrors the sync native
                // path's `hook_call(true, nargs)` site
                // (`begin_call`) so embedders with a
                // Rust debug hook installed see a Call event
                // for async natives identical to the sync
                // path. The matching "return" hook fires from
                // `commit_async_native_result` in
                // `async_drive.rs` after the future resolves.
                // Placement: after the `native_nresults` / `gc_top`
                // pin, before the future is constructed, so a
                // hook body that triggers GC observes the
                // correct pinned window. On hook error the
                // sentinel never returns and
                // `pending_async_native_*` remain `None` —
                // the executor sees `DispatchOutcome::Error`.
                if let Err(e) = self.hook_call(true, nargs) {
                    return Some(Err(e));
                }
                // Transmute the stored NativeFn back to its
                // real AsyncNativeFn shape. Sound because
                // `set_async_native` / `create_async_native`
                // installed an AsyncNativeFn through the
                // identically-sized fn-pointer slot, and the
                // `is_async` marker bit is what records that
                // fact.
                let async_fn: crate::vm::async_drive::AsyncNativeFn =
                    // SAFETY: same-size fn pointers; provenance
                    // preserved through `mem::transmute`. The
                    // `is_async` marker is the only safe-to-call
                    // gate, set exclusively by
                    // `Vm::create_async_native`.
                    unsafe { std::mem::transmute(nc.f) };
                let vm_ptr: *mut Vm = self;
                let fut = async_fn(vm_ptr, func_slot, nargs);
                // Stash the future + post-call context for
                // `drive_one` to surface to `EvalFuture::poll`.
                self.pending_async_native_fut = Some(fut);
                self.pending_async_native_ctx = Some(AsyncNativeCallCtx {
                    func_slot,
                    nargs,
                    nresults,
                    gc_top: self.gc_top,
                });
                // Sentinel Err walked up to `drive_one` (same
                // shape as `host_yield_pending`'s budget yield).
                // Value::Nil — never seen by user code.
                Some(Err(LuaError(Value::Nil)))
            }
            // pcall/xpcall are yieldable: rather than calling the
            // protected function through the Rust stack (which cannot be
            // suspended), push a continuation frame and drive the call
            // through the interpreter loop (PUC lua_pcallk). A yield
            // inside it is preserved with the thread's saved frames.
            NativeKind::Pcall => Some(self.begin_pcall(func_slot, nargs, nresults)),
            // 5.1 `xpcall(f, err)` calls `f` with no arguments
            NativeKind::Xpcall => {
                let forward = self.version > LuaVersion::Lua51;
                Some(self.begin_xpcall(func_slot, nargs, nresults, forward))
            }
            NativeKind::HostXpcall => Some(self.begin_xpcall(func_slot, nargs, nresults, true)),
            // From 5.4 on, pairs(t) calls a __pairs metamethod yieldably
            // (PUC luaB_pairs uses lua_callk). 5.2/5.3 use a plain
            // lua_call, and 5.1 has no `__pairs`: the native handles those.
            NativeKind::Pairs => {
                if nargs >= 1 && self.version >= LuaVersion::Lua54 {
                    let arg = self.stack[(func_slot + 1) as usize];
                    if !self.get_mm(arg, Mm::Pairs).is_nil() {
                        return Some(self.begin_pairs(func_slot, nresults));
                    }
                }
                None
            }
            NativeKind::Plain => None,
        }
    }
}
