//! Cooperative-yield core for `Vm::eval_async`.
//!
//! This module implements:
//!
//! - `DispatchOutcome` — terminal / cooperative-yield enum.
//! - `Vm::drive_one` — runs the dispatcher until completion / error /
//!   `BudgetExhausted`. Layers on `Vm::call_value` for the bootstrap
//!   poll and on `Vm::exec_with_async` for resume polls.
//! - [`EvalFuture`] — `!Send` `std::future::Future` that owns the
//!   `&mut Vm` borrow and surfaces the poll loop.
//! - [`Vm::eval_async`] / [`Vm::eval_async_chunk`] — public entry
//!   points; convenience for embedders wanting `tokio` / `async-std`
//!   integration.
//!
//! Async mode auto-disables JIT for the future's lifetime and
//! restores the prior setting on terminal poll.
//!
//! ```
//! use luna_core::vm::Vm;
//! use luna_core::version::LuaVersion;
//! use std::future::Future;
//! use std::pin::Pin;
//! use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
//!
//! // 20-line hand-rolled block_on (no tokio dep).
//! fn block_on<F: Future>(mut fut: F) -> F::Output {
//!     fn raw_waker() -> RawWaker {
//!         fn noop(_: *const ()) {}
//!         fn clone(_: *const ()) -> RawWaker { raw_waker() }
//!         static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
//!         RawWaker::new(std::ptr::null(), &VT)
//!     }
//!     let waker = unsafe { Waker::from_raw(raw_waker()) };
//!     let mut cx = Context::from_waker(&waker);
//!     let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
//!     loop {
//!         match fut.as_mut().poll(&mut cx) {
//!             Poll::Ready(v) => return v,
//!             Poll::Pending => continue,
//!         }
//!     }
//! }
//!
//! let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
//! let r = block_on(vm.eval_async("return 1 + 2")).unwrap();
//! assert_eq!(r.len(), 1);
//! ```

use crate::runtime::Value;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Async-native function ABI. Returns a
/// `Pin<Box<dyn Future>>` that resolves to the return-value count
/// (same convention as sync [`crate::runtime::value::NativeFn`]: write
/// results into the caller's slot via the borrowed `Vm`, then yield
/// the count back).
///
/// # Safety contract
///
/// The first parameter is `*mut Vm` rather than `&mut Vm` because the
/// returned `Pin<Box<dyn Future>>` is `'static` (the trait object
/// erases lifetimes) and we cannot tie it to the caller's borrow
/// without `for<'vm>` HRTBs that the trait system rejects on `dyn`
/// futures. Implementors must reborrow inside the future:
///
/// ```ignore
/// fn my_async(
///     vm: *mut Vm,
///     func_slot: u32,
///     nargs: u32,
/// ) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
///     Box::pin(async move {
///         // SAFETY: the dispatcher is suspended and EvalFuture
///         // holds the unique &mut Vm borrow for the future's
///         // entire lifetime; no concurrent access can occur.
///         let vm = unsafe { &mut *vm };
///         // ... read args from vm.stack[func_slot+1..], do async
///         //     work (e.g. `sleep(...).await`), write results back
///         //     to vm.stack[func_slot..], return their count ...
///         Ok(0)
///     })
/// }
/// ```
///
/// The `Vm` is exclusively owned by the active [`EvalFuture`] for the
/// suspension's full lifetime (the dispatcher is paused; the host's
/// executor is the only driver). This makes the `unsafe { &mut *vm }`
/// reborrow sound provided the future doesn't leak the borrow past
/// its own `await` boundaries.
///
/// The native is invoked exactly once per Lua call site. The future
/// is polled by [`EvalFuture::poll`]; on `Poll::Ready(Ok(n))` the
/// dispatcher resumes, treats slots `[func_slot, func_slot+n)` as the
/// return list, and continues. On `Poll::Ready(Err(e))` the error
/// propagates as if a sync native had returned it.
pub type AsyncNativeFn =
    fn(*mut Vm, func_slot: u32, nargs: u32) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>>;

/// Outcome of a single dispatcher slice driven by
/// [`Vm::drive_one`]. The `AsyncNativeAwaiting` variant is
/// for async natives: the dispatcher suspends in-place, hands the
/// returned future to [`EvalFuture::poll`], and resumes the same call
/// site once the future resolves.
pub(crate) enum DispatchOutcome {
    /// The chunk returned cleanly; values are the Lua-side return list.
    Complete(Vec<Value>),
    /// A genuine runtime / syntax / type error (NOT a budget yield).
    Error(LuaError),
    /// The per-poll instruction quota was exhausted. The dispatcher's
    /// call frames are intact; the next [`Vm::drive_one`] call (after
    /// the host pumps the executor) resumes from the same point.
    BudgetExhausted,
    /// The dispatcher invoked an async-marked
    /// native; the returned future is now under host drive. The Vm
    /// preserves the in-flight call's `(func_slot, nargs, nresults)`
    /// context in `pending_async_native_ctx` so that
    /// [`Vm::commit_async_native_result`] can land the future's
    /// eventual `Ok(nret)` back into the calling frame.
    AsyncNativeAwaiting(Pin<Box<dyn Future<Output = Result<u32, LuaError>>>>),
}

impl Vm {
    /// Allocate a `Value::Native` whose closure is
    /// tagged as async (`NativeClosure.is_async = true`). The
    /// underlying `NativeFn` pointer slot stores `f` transmuted from
    /// [`AsyncNativeFn`] — same pointer width, no provenance loss —
    /// and the marker bit is what tells the dispatcher to route it
    /// through the cooperative-yield path.
    ///
    /// The returned `Value` can be installed under a Lua global via
    /// [`Vm::set_global`], passed as a callback, stored in a table —
    /// whatever a sync `vm.native(f)` value supports. Calling it from
    /// a sync `Vm::eval` context raises `LuaError` ("async native
    /// called in sync context"); only `Vm::eval_async` (or another
    /// driver that sets `async_mode = true`) can drive it.
    pub fn create_async_native(&mut self, f: AsyncNativeFn) -> Value {
        // SAFETY: `AsyncNativeFn` and `NativeFn` are both Rust `fn`
        // pointers and have identical size + alignment (single word).
        // The `is_async` marker bit, set by `Heap::new_async_native`,
        // is the discriminant the dispatcher reads before transmuting
        // back to `AsyncNativeFn` at the call site; without the bit
        // the pointer is never invoked.
        let raw_fn: crate::runtime::value::NativeFn = unsafe { std::mem::transmute(f) };
        Value::Native(self.heap.new_async_native(raw_fn, Box::new([])))
    }

    /// Convenience: install an async native under
    /// `name` as a Lua global. Equivalent to
    /// `vm.set_global(name, vm.create_async_native(f))`.
    pub fn set_async_native(&mut self, name: &str, f: AsyncNativeFn) -> Result<(), LuaError> {
        let v = self.create_async_native(f);
        self.set_global(name, v)
    }

    /// Convenience entry: compile + run `src` as an
    /// anonymous chunk via the cooperative-yield dispatcher. The
    /// returned `EvalFuture` borrows `&mut self` for its full lifetime,
    /// which (by `Vm: !Send`) keeps it pinned to a single OS thread.
    ///
    /// Holding two `EvalFuture`s on the same Vm is blocked by the
    /// borrow checker (`&mut Vm` exclusivity). Holding a sync
    /// `eval`/`call_value` call *while* an `EvalFuture` is in flight
    /// is likewise blocked.
    ///
    /// The chunk source name in tracebacks is `"=eval"`. Use
    /// [`Vm::eval_async_chunk`] to supply a custom name.
    pub fn eval_async<'vm>(&'vm mut self, src: &str) -> EvalFuture<'vm> {
        self.eval_async_chunk(src, "=eval")
    }

    /// Like [`Vm::eval_async`] but with a
    /// user-supplied chunk name (appears in tracebacks).
    pub fn eval_async_chunk<'vm>(&'vm mut self, src: &str, name: &str) -> EvalFuture<'vm> {
        EvalFuture::new(self, src, name)
    }

    /// Set the per-poll opcode quota loaded into
    /// `instr_budget` at the start of each [`EvalFuture`] poll slice.
    /// Default 10_000 opcodes. Smaller = finer-grained cooperative
    /// yield (lower per-task latency, more task-switch overhead);
    /// larger = closer to sync throughput per slice.
    pub fn set_async_slice(&mut self, n: i64) {
        // i64::MAX silently caps at i64::MAX; non-positive values
        // would loop indefinitely so clamp to 1 (a single opcode per
        // slice — pathological but well-defined).
        self.async_slice_size = n.max(1);
    }

    /// Current per-poll async slice size (default
    /// 10_000).
    pub fn async_slice(&self) -> i64 {
        self.async_slice_size
    }

    /// Drive the dispatcher one slice. Used
    /// internally by [`EvalFuture::poll`]. The `bootstrap` flag tells
    /// the helper whether this is the first slice of a fresh chunk
    /// (in which case `call_value` sets up the call frame) or a
    /// resume (in which case the existing frames live in `self.frames`
    /// and the helper just re-enters the dispatcher at the saved
    /// `entry_depth`).
    pub(crate) fn drive_one(
        &mut self,
        bootstrap: Option<Value>,
        entry_depth: usize,
    ) -> DispatchOutcome {
        // Arm `async_mode` so the budget hot loop yields cooperatively
        // instead of erroring. The future installs this once on the
        // first poll and clears it on terminal poll; arming again here
        // is idempotent.
        self.async_mode = true;
        // Arm a fresh slice quota. The previous slice exhausted to 0;
        // `instr_budget` was set to `None` by the hot loop on
        // exhaustion. Reload it for this slice.
        self.instr_budget = Some(self.async_slice_size);

        let raw = match bootstrap {
            Some(closure_val) => {
                // First slice — set up the call frame via the existing
                // `call_value` path. This handles `c_depth`,
                // `public_call_depth`, `clear_error_metadata`, and the
                // `begin_call` push. On a synchronous completion (e.g.
                // a chunk whose only op is `return`) the call
                // finishes within `call_value` and we hit
                // `Complete` immediately.
                self.call_value(closure_val, &[])
            }
            None => {
                // Resume slice — frames are intact from the prior
                // `BudgetExhausted`. Walk the dispatcher again.
                self.exec_with_async(entry_depth)
            }
        };

        match raw {
            Ok(values) => DispatchOutcome::Complete(values),
            Err(e) => {
                // Async-native suspension takes
                // precedence: the future is the active work item, the
                // sentinel Err is just transport. Check before
                // `host_yield_pending` because both flags can in
                // principle coexist (a budget exhaustion deferred by
                // an in-flight async-native call) but the async-native
                // future must be drained first.
                if self.pending_async_native_fut.is_some() {
                    let fut = self.pending_async_native_fut.take().expect("checked above");
                    // ctx stays in place — `commit_async_native_result`
                    // consumes it when the future resolves.
                    DispatchOutcome::AsyncNativeAwaiting(fut)
                } else if self.host_yield_pending {
                    self.host_yield_pending = false;
                    DispatchOutcome::BudgetExhausted
                } else {
                    DispatchOutcome::Error(e)
                }
            }
        }
    }

    /// Land an async native's resolved return
    /// count back into the calling frame's expected result slots.
    /// Mirrors the sync-native tail of `call_at` (sans the
    /// `running_natives` bookkeeping). Consumes
    /// `Vm.pending_async_native_ctx`; subsequent `drive_one` calls
    /// resume the dispatcher above this call site.
    ///
    /// Called by [`EvalFuture::poll`] after the awaited future
    /// resolves to `Poll::Ready(Ok(nret))`.
    pub(crate) fn commit_async_native_result(&mut self, nret: u32) -> Result<(), LuaError> {
        let ctx = self
            .pending_async_native_ctx
            .take()
            .expect("commit_async_native_result without a pending ctx");
        self.finish_results(ctx.func_slot, nret, ctx.nresults);
        // Fire the matching "return" hook for the
        // async native, after results land in the call window and
        // before the post-call GC checkpoint. Mirrors the sync
        // native's `hook_return(true, nargs + 1, nret)` placement in
        // `exec.rs`. The sync path widens its C-frame argument window
        // around the hook so `debug.getlocal(2, ftransfer..)` reads
        // the results; the async path doesn't push to
        // `running_natives` (the future owned the borrow window
        // across `.await`), so there's no `running_native_slots` to
        // widen — `hook_ftransfer` / `hook_ntransfer` set by
        // `hook_return` carry the same information for Rust hooks
        // and for Lua hooks reading `debug.getinfo(.).ftransfer`.
        let ftransfer = ctx.nargs + 1;
        self.hook_return(true, ftransfer, nret)?;
        // Same post-call GC checkpoint the sync path runs: the native
        // may have allocated, and the live boundary is now the result
        // window.
        self.maybe_collect_garbage(self.top);
        Ok(())
    }
}

#[path = "async_future.rs"]
mod future;
pub use future::EvalFuture;
