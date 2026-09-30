//! [`EvalFuture`]: the poll loop of [`Vm::eval_async`].

use super::*;

/// Host-driven cooperative-yield future. Borrows
/// `&mut Vm` for its full lifetime; the borrow + `Vm: !Send` together
/// make the future `!Send` (suits tokio `current_thread` /
/// `LocalSet`, NOT multi-thread runtimes).
///
/// See module docs for a hand-rolled `block_on` usage example.
pub struct EvalFuture<'vm> {
    vm: &'vm mut Vm,
    state: EvalState,
    /// Saved `jit.enabled` snapshot from the first poll. JIT-compiled
    /// traces don't honor `instr_budget` at every opcode, so a runaway
    /// trace in async mode could starve other tokio tasks. The future
    /// disables JIT for its duration and restores on terminal poll
    /// (or on Drop).
    saved_jit_enabled: Option<bool>,
    /// Saved `async_slice_size`. The future doesn't mutate it; the
    /// field lets an async-native path install per-future slice tweaks
    /// without leaking them into sibling futures.
    #[allow(dead_code)]
    saved_async_slice: Option<i64>,
}

/// State machine driving an `EvalFuture`.
///
/// - `Initial` — pre-compile. The source string is owned so the
///   future can outlive the caller's `&str`.
/// - `Running` — bootstrap done; subsequent polls resume from
///   `entry_depth`.
/// - `Done` — terminal. Polling again panics (per `Future` contract:
///   futures must not be polled after `Poll::Ready`).
impl<'vm> EvalFuture<'vm> {
    pub(super) fn new(vm: &'vm mut Vm, src: &str, name: &str) -> Self {
        EvalFuture {
            vm,
            state: EvalState::Initial {
                src: src.to_string(),
                name: name.to_string(),
            },
            saved_jit_enabled: None,
            saved_async_slice: None,
        }
    }
}

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
    /// An async native is mid-await. The future is
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
                // Stash JIT setting + disable for the duration (JIT
                // traces don't honor instr_budget per opcode, so async
                // mode + JIT could starve the executor).
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
                        // (error classification + source position).
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
                    // Register the waker so an in-flight async native can
                    // wake the host. A budget exhaustion re-wakes the host
                    // immediately via `cx.waker().wake_by_ref()`.
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
                            // Re-wake immediately so the host's executor polls
                            // us again. The `wake_by_ref` call models "we still
                            // have work to do but want to let other tasks run".
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
                            // Commit may fire the
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
        // not done here; there is no `Vm::cancel_async`. Embedders
        // relying on cancellation should construct a fresh Vm per request.
        if let Some(prev) = self.saved_jit_enabled.take() {
            self.vm.set_jit_enabled(prev);
        }
        // Always clear async state on drop so the next `eval` / `eval_async`
        // call on the same Vm starts clean.
        self.vm.async_mode = false;
        self.vm.async_waker = None;
        self.vm.host_yield_pending = false;
        // Async-native bookkeeping. The future is
        // owned by `EvalFuture` (not by the Vm) once `drive_one`
        // surfaces it, so cancelling here only needs to clear the
        // post-call ctx; the dropped EvalFuture takes the Pin<Box<...>>
        // with it.
        self.vm.pending_async_native_fut = None;
        self.vm.pending_async_native_ctx = None;
    }
}
