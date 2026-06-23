//! v1.1 B10 Stage 1 — cooperative-yield core for `Vm::eval_async`.
//!
//! See `.dev/rfcs/v1.1-rfc-b10-async-embedder.md` (§D1, §D2, §D4, §D5,
//! §D8) for the full design. This module implements the Stage 1 slice:
//!
//! - [`DispatchOutcome`] — terminal / cooperative-yield enum.
//! - [`Vm::drive_one`] — runs the dispatcher until completion / error /
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

/// v1.1 B10 Stage 1 — outcome of a single dispatcher slice driven by
/// [`Vm::drive_one`]. Stage 2 introduces an additional
/// `AsyncNativeAwaiting` variant for async natives; Stage 1 stops at
/// the three variants below.
#[derive(Debug)]
pub(crate) enum DispatchOutcome {
    /// The chunk returned cleanly; values are the Lua-side return list.
    Complete(Vec<Value>),
    /// A genuine runtime / syntax / type error (NOT a budget yield).
    Error(LuaError),
    /// The per-poll instruction quota was exhausted. The dispatcher's
    /// call frames are intact; the next [`Vm::drive_one`] call (after
    /// the host pumps the executor) resumes from the same point.
    BudgetExhausted,
}

impl Vm {
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
                if self.host_yield_pending {
                    self.host_yield_pending = false;
                    DispatchOutcome::BudgetExhausted
                } else {
                    DispatchOutcome::Error(e)
                }
            }
        }
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
                        this.vm.set_error_kind(crate::vm::error::LuaErrorKind::Syntax);
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
    }
}
