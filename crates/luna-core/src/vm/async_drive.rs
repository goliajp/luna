//! v1.1 B10 Stage 1 — cooperative-yield core for `Vm::eval_async`.
//!
//! See `.dev/rfcs/v1.1-rfc-b10-async-embedder.md` (§D1, §D2, §D4, §D5,
//! §D8) for the full design. This module implements the Stage 1 slice:
//!
//! - `DispatchOutcome` — terminal / cooperative-yield enum.
//! - `Vm::drive_one` — runs the dispatcher until completion / error /
//!   `BudgetExhausted`. Layers on `Vm::call_value` for the bootstrap
//!   poll and on `Vm::exec_with_async` for resume polls.
//! - [`EvalFuture`] — `!Send` `std::future::Future` that owns the
//!   `&mut Vm` borrow and surfaces the poll loop of RFC §D4.
//! - [`Vm::eval_async`] / [`Vm::eval_async_chunk`] — public entry
//!   points; convenience for embedders wanting `tokio` / `async-std`
//!   integration.
//!
//! Stage 1 deliberately does NOT touch the JIT layer: async mode
//! auto-disables JIT for the future's lifetime (RFC "Risks") and
//! restores the prior setting on terminal poll. Async natives, the
//! `Lua` facade `eval_async`, and `examples/async_host.rs` land in
//! Stage 2/3/4.
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

/// v1.1 B10 Stage 2 — async-native function ABI. Returns a
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

/// v1.1 B10 Stage 1 — outcome of a single dispatcher slice driven by
/// [`Vm::drive_one`]. Stage 2 adds the `AsyncNativeAwaiting` variant
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
    /// v1.1 B10 Stage 2 — the dispatcher invoked an async-marked
    /// native; the returned future is now under host drive. The Vm
    /// preserves the in-flight call's `(func_slot, nargs, nresults)`
    /// context in `pending_async_native_ctx` so that
    /// [`Vm::commit_async_native_result`] can land the future's
    /// eventual `Ok(nret)` back into the calling frame.
    AsyncNativeAwaiting(Pin<Box<dyn Future<Output = Result<u32, LuaError>>>>),
}

impl Vm {
    /// v1.1 B10 Stage 2 — allocate a `Value::Native` whose closure is
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

    /// v1.1 B10 Stage 2 — convenience: install an async native under
    /// `name` as a Lua global. Equivalent to
    /// `vm.set_global(name, vm.create_async_native(f))`.
    pub fn set_async_native(&mut self, name: &str, f: AsyncNativeFn) -> Result<(), LuaError> {
        let v = self.create_async_native(f);
        self.set_global(name, v)
    }

    /// v1.1 B10 Stage 1 — convenience entry: compile + run `src` as an
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

    /// v1.1 B10 Stage 1 — like [`Vm::eval_async`] but with a
    /// user-supplied chunk name (appears in tracebacks).
    pub fn eval_async_chunk<'vm>(&'vm mut self, src: &str, name: &str) -> EvalFuture<'vm> {
        EvalFuture {
            vm: self,
            state: EvalState::Initial {
                src: src.to_string(),
                name: name.to_string(),
            },
            saved_jit_enabled: None,
            saved_async_slice: None,
        }
    }

    /// v1.1 B10 Stage 1 — set the per-poll opcode quota loaded into
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

    /// v1.1 B10 Stage 1 — current per-poll async slice size (default
    /// 10_000).
    pub fn async_slice(&self) -> i64 {
        self.async_slice_size
    }

    /// v1.1 B10 Stage 1 — drive the dispatcher one slice. Used
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
                // v1.1 B10 Stage 2 — async-native suspension takes
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

    /// v1.1 B10 Stage 2 — land an async native's resolved return
    /// count back into the calling frame's expected result slots.
    /// Mirrors the sync-native tail of `call_at` (sans hooks +
    /// `running_natives` bookkeeping, which Stage 2 deliberately skips
    /// — see RFC §"Risks"). Consumes
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
        // v1.3 Phase AS — fire the matching "return" hook for the
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

/// v1.1 B10 Stage 1 — host-driven cooperative-yield future. Borrows
/// `&mut Vm` for its full lifetime; the borrow + `Vm: !Send` together
/// make the future `!Send` (suits tokio `current_thread` /
/// `LocalSet`, NOT multi-thread runtimes).
///
/// See module docs for the RFC reference and a hand-rolled `block_on`
/// usage example.
pub struct EvalFuture<'vm> {
    vm: &'vm mut Vm,
    state: EvalState,
    /// Saved `jit.enabled` snapshot from the first poll. JIT-compiled
    /// traces don't honor `instr_budget` at every opcode (per
    /// `v1.1-audit-async.md` §"JIT trace yield"), so a runaway trace
    /// in async mode could starve other tokio tasks. The future
    /// disables JIT for its duration and restores on terminal poll
    /// (or on Drop).
    saved_jit_enabled: Option<bool>,
    /// Saved `async_slice_size` is unused in Stage 1 (we don't mutate
    /// it from inside the future), but the field is here so Stage 2's
    /// async-native path can install per-future slice tweaks without
    /// leaking them into sibling futures.
    #[allow(dead_code)]
    saved_async_slice: Option<i64>,
}

/// v1.1 B10 Stage 1 — three-state machine driving an `EvalFuture`.
///
/// - `Initial` — pre-compile. The source string is owned so the
///   future can outlive the caller's `&str`.
/// - `Running` — bootstrap done; subsequent polls resume from
///   `entry_depth`.
/// - `Done` — terminal. Polling again panics (per `Future` contract:
///   futures must not be polled after `Poll::Ready`).
enum EvalState {
    Initial {
        src: String,
        name: String,
    },
    Running {
        entry_depth: usize,
        /// `true` only on the very first slice — we still need to
        /// invoke `call_value` to push the entry frame. After the
        /// first `BudgetExhausted`, this flips to `false` and the
        /// future resumes via `exec_with_async`.
        first_slice: bool,
        /// Cached for `bootstrap = Some(...)`. After bootstrap fires
        /// once, the value is `None`.
        closure: Option<Value>,
    },
    /// v1.1 B10 Stage 2 — an async native is mid-await. The future is
    /// owned here (rather than on the `Vm`) so an explicit `Drop` of
    /// `EvalFuture` cancels the in-flight future cleanly. On the next
    /// poll: if the future resolves to `Ok(nret)`, the EvalFuture
    /// calls `Vm::commit_async_native_result(nret)` and falls back to
    /// `EvalState::Running` to keep driving the dispatcher; on `Err`
    /// the EvalFuture transitions to `Done` and surfaces the error.
    AwaitingNative {
        entry_depth: usize,
        fut: Pin<Box<dyn Future<Output = Result<u32, LuaError>>>>,
    },
    Done,
}

impl<'vm> Future for EvalFuture<'vm> {
    type Output = Result<Vec<Value>, LuaError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // `EvalFuture` holds no self-referential state — `vm` is a
        // plain mutable borrow, `state` is owned by value. Safe to
        // project out of the pin without `pin-project`.
        let this = unsafe { self.as_mut().get_unchecked_mut() };

        loop {
            // ---- State transition: Initial → Running ----
            if let EvalState::Initial { src, name } = &this.state {
                // Stash JIT setting + disable for the duration (RFC
                // §"Risks": JIT traces don't honor instr_budget per
                // opcode, so async mode + JIT could starve the
                // executor).
                if this.saved_jit_enabled.is_none() {
                    this.saved_jit_enabled = Some(this.vm.jit_enabled());
                    this.vm.set_jit_enabled(false);
                }
                // Compile. On syntax error we transition directly to
                // Done with the error — no Lua frames were pushed,
                // so the Vm is back at quiescent state.
                let cl = match this.vm.load(src.as_bytes(), name.as_bytes()) {
                    Ok(c) => c,
                    Err(syntax) => {
                        // Match `eval_chunk`'s syntax-error shaping
                        // (B6 classification + source position).
                        this.vm
                            .set_error_kind(crate::vm::error::LuaErrorKind::Syntax);
                        this.vm.set_error_source(name.clone(), syntax.line);
                        let msg = format!("{}", syntax);
                        let s = this.vm.intern_str(&msg);
                        // Restore JIT + clean up before returning.
                        if let Some(prev) = this.saved_jit_enabled.take() {
                            this.vm.set_jit_enabled(prev);
                        }
                        this.vm.async_mode = false;
                        this.vm.async_waker = None;
                        this.state = EvalState::Done;
                        return Poll::Ready(Err(LuaError(Value::Str(s))));
                    }
                };
                // For the bootstrap slice, frames.len() is currently
                // 0 (no prior calls on this Vm: enforced by `&mut
                // Vm` exclusivity over the future's lifetime). The
                // `call_value` path will push one Lua frame, so the
                // saved `entry_depth` is 1. We capture it explicitly
                // rather than reading `vm.frames.len()` post-call so
                // resume after BudgetExhausted reuses the right
                // depth.
                let entry_depth = this.vm.frame_count().saturating_add(1);
                this.state = EvalState::Running {
                    entry_depth,
                    first_slice: true,
                    closure: Some(Value::Closure(cl)),
                };
                // Fall through to Running.
            }

            // ---- State: Running. Drive a slice. ----
            match &mut this.state {
                EvalState::Running {
                    entry_depth,
                    first_slice,
                    closure,
                } => {
                    // Register the waker for Stage 2's wakeup
                    // mechanism (Stage 1 always re-wakes the host
                    // immediately on BudgetExhausted via
                    // `cx.waker().wake_by_ref()`, so this is
                    // forward-looking).
                    this.vm.async_waker = Some(cx.waker().clone());

                    let (bootstrap_arg, ed) = if *first_slice {
                        (closure.take(), *entry_depth)
                    } else {
                        (None, *entry_depth)
                    };
                    let ed_for_resume = *entry_depth;
                    let outcome = this.vm.drive_one(bootstrap_arg, ed);
                    // The first slice is consumed.
                    *first_slice = false;

                    match outcome {
                        DispatchOutcome::Complete(values) => {
                            // Restore JIT + clear async state.
                            if let Some(prev) = this.saved_jit_enabled.take() {
                                this.vm.set_jit_enabled(prev);
                            }
                            this.vm.async_mode = false;
                            this.vm.async_waker = None;
                            this.state = EvalState::Done;
                            return Poll::Ready(Ok(values));
                        }
                        DispatchOutcome::Error(e) => {
                            if let Some(prev) = this.saved_jit_enabled.take() {
                                this.vm.set_jit_enabled(prev);
                            }
                            this.vm.async_mode = false;
                            this.vm.async_waker = None;
                            this.state = EvalState::Done;
                            return Poll::Ready(Err(e));
                        }
                        DispatchOutcome::BudgetExhausted => {
                            // Stage 1: re-wake immediately so the
                            // host's executor polls us again. Stage 2
                            // can wait for an async native's waker
                            // before re-polling. The `wake_by_ref`
                            // call models "we still have work to do
                            // but want to let other tasks run".
                            cx.waker().wake_by_ref();
                            return Poll::Pending;
                        }
                        DispatchOutcome::AsyncNativeAwaiting(fut) => {
                            // Stash the future + flip to AwaitingNative.
                            // Loop back to the top so the very next
                            // iteration polls it (gives Ready-fast
                            // futures a one-poll completion path).
                            this.state = EvalState::AwaitingNative {
                                entry_depth: ed_for_resume,
                                fut,
                            };
                            continue;
                        }
                    }
                }
                EvalState::AwaitingNative { entry_depth, fut } => {
                    // Poll the in-flight async native. On Ready, land
                    // the result into the calling Lua frame and fall
                    // back into Running so `drive_one` resumes the
                    // dispatcher above this call site. On Pending,
                    // surface to the host — the future itself
                    // registered any wakers it needs inside the host
                    // executor (e.g. a tokio timer).
                    match fut.as_mut().poll(cx) {
                        Poll::Ready(Ok(nret)) => {
                            let ed = *entry_depth;
                            // v1.3 Phase AS — commit may fire the
                            // async-native "return" hook, which can
                            // error (hook propagates `LuaError`). On
                            // error, run the same cleanup the
                            // `Poll::Ready(Err)` arm runs below.
                            if let Err(e) = this.vm.commit_async_native_result(nret) {
                                if let Some(prev) = this.saved_jit_enabled.take() {
                                    this.vm.set_jit_enabled(prev);
                                }
                                this.vm.async_mode = false;
                                this.vm.async_waker = None;
                                this.state = EvalState::Done;
                                return Poll::Ready(Err(e));
                            }
                            this.state = EvalState::Running {
                                entry_depth: ed,
                                first_slice: false,
                                closure: None,
                            };
                            continue;
                        }
                        Poll::Ready(Err(e)) => {
                            // Drop the in-flight ctx — the future
                            // failed, so its slot is gone.
                            this.vm.pending_async_native_ctx = None;
                            if let Some(prev) = this.saved_jit_enabled.take() {
                                this.vm.set_jit_enabled(prev);
                            }
                            this.vm.async_mode = false;
                            this.vm.async_waker = None;
                            this.state = EvalState::Done;
                            return Poll::Ready(Err(e));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                EvalState::Initial { .. } => unreachable!("transitioned above"),
                EvalState::Done => panic!("EvalFuture polled after Poll::Ready"),
            }
        }
    }
}

impl<'vm> Drop for EvalFuture<'vm> {
    fn drop(&mut self) {
        // If the future is dropped mid-flight (host timeout, task
        // cancelled), restore any state we mutated so the Vm is
        // usable again. Note: stale call frames from an in-flight
        // chunk remain in `vm.frames`; a full cleanup pass (closing
        // `__close` handlers etc.) would mirror `close_coro` and is
        // out of scope for Stage 1 — the RFC defers
        // `Vm::cancel_async` to a follow-up. Embedders relying on
        // cancellation should construct a fresh Vm per request.
        if let Some(prev) = self.saved_jit_enabled.take() {
            self.vm.set_jit_enabled(prev);
        }
        // Always clear async state on drop so the next `eval` / `eval_async`
        // call on the same Vm starts clean.
        self.vm.async_mode = false;
        self.vm.async_waker = None;
        self.vm.host_yield_pending = false;
        // v1.1 B10 Stage 2 — async-native bookkeeping. The future is
        // owned by `EvalFuture` (not by the Vm) once `drive_one`
        // surfaces it, so cancelling here only needs to clear the
        // post-call ctx; the dropped EvalFuture takes the Pin<Box<...>>
        // with it.
        self.vm.pending_async_native_fut = None;
        self.vm.pending_async_native_ctx = None;
    }
}
