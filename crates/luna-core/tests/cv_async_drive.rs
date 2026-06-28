//! v2.0 Phase 5 CV gap fill — `vm::async_drive` paths not covered by
//! the existing `eval_async.rs` smoke set.
//!
//! Existing coverage in `eval_async.rs` exercises the happy path
//! (compile-err / runtime-err / long-loop yield / multi-value /
//! slice accessors / sync-after-async). This file targets the
//! audit-flagged gaps:
//!
//! 1. `EvalFuture::drop` mid-execution restores `jit_enabled` and
//!    clears `async_mode` / `async_waker` so the Vm is reusable.
//! 2. `eval_async_chunk(name)` passes the chunk name into the
//!    syntax-error path (visible via `error_source`).
//! 3. After a syntax error completes via the Initial → Done
//!    transition, the Vm state is fully restored (no leaked
//!    `async_mode`, JIT setting back to original).
//! 4. `eval_async` honors a non-default JIT setting on entry +
//!    restores it on terminal poll (verifies the snapshot/restore
//!    invariant in both states: enabled-before and disabled-before).
//! 5. A future polled after `Ready` panics per `Future` contract.
//!
//! No `tokio` dep — hand-rolled `block_on` plus a single-step poll
//! helper (no `Arc`-leaking counting waker required; gap tests need
//! "poll once and stop" + "block until ready" only).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

// ───────── Hand-rolled executor (no tokio dep) ─────────

fn noop_waker() -> Waker {
    fn noop(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        raw()
    }
    fn raw() -> RawWaker {
        static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        RawWaker::new(std::ptr::null(), &VT)
    }
    unsafe { Waker::from_raw(raw()) }
}

fn block_on<F: Future>(mut fut: F) -> F::Output {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => continue,
        }
    }
}

// ───────── Tests ─────────

/// Drop an `EvalFuture` while the dispatcher is still grinding
/// through a long Lua loop, then verify the Vm is back to a clean
/// post-drop state — `jit_enabled` restored to its pre-future
/// value, and a subsequent `eval` works.
///
/// This is the cancellation-safety promise the `Drop` impl in
/// `async_drive.rs:558` commits to.
#[test]
fn eval_future_drop_mid_execution_restores_state() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();

    // Capture the entry JIT setting. We need to know what restore
    // should land on.
    let jit_before = vm.jit_enabled();

    // Tight slice so the future absolutely yields on the first poll
    // (BudgetExhausted) without completing.
    vm.set_async_slice(1);
    {
        // Single-poll then drop. We can't use block_on here — that
        // would drive to completion. Poll once and let `fut` drop
        // at the end of this block.
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut fut = vm.eval_async("local s=0 for i=1,1000000 do s=s+i end return s");
        let mut pinned = unsafe { Pin::new_unchecked(&mut fut) };
        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(_) => {
                panic!("expected Pending after 1-opcode slice on a million-iter loop; got Ready")
            }
            Poll::Pending => {} // expected
        }
        // `fut` (and the borrow on `vm`) drops here. Drop impl must
        // restore `jit_enabled` + clear `async_mode` + clear
        // `async_waker`.
    }

    // jit_enabled snapped back to original.
    assert_eq!(
        vm.jit_enabled(),
        jit_before,
        "EvalFuture::drop must restore the pre-future jit_enabled value"
    );

    // A fresh sync eval must work — proves async_mode / waker are
    // cleared (otherwise the dispatcher would refuse or misbehave).
    // Note: orphaned Lua call frames from the dropped chunk may
    // linger per the Stage-1 RFC defer; we exercise a fresh chunk
    // that doesn't depend on a quiescent call stack.
    let r = vm.eval("return 21 + 21").expect("sync eval after drop");
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Value::Int(42)));
}

/// `eval_async_chunk(name)` surfaces the chunk name in the error
/// classifier on a syntax error. Existing `eval_async.rs` only
/// exercises the default `"=eval"` name path.
#[test]
fn eval_async_chunk_custom_name_in_syntax_error() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let err = block_on(vm.eval_async_chunk("function function", "=my_chunk")).unwrap_err();
    let msg = vm.error_text(&err);
    assert!(!msg.is_empty(), "syntax error must have non-empty message");
    assert_eq!(
        vm.error_kind(),
        luna_core::vm::LuaErrorKind::Syntax,
        "expected Syntax classification, got {:?}",
        vm.error_kind()
    );
    // The error_source machinery on the Vm is set by the Initial →
    // Done transition (see async_drive.rs:407). The chunk name
    // should be the one we passed, not the "=eval" default.
    let (name, _line) = vm.error_source().expect("error_source must be populated");
    assert_eq!(name, "=my_chunk");
}

/// After a syntax-error completes (Initial → Done path, no Running
/// state visited), the Vm is back to a fully clean state — a fresh
/// `eval_async` works AND the JIT setting is restored.
#[test]
fn eval_async_syntax_error_cleans_state_for_reuse() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let jit_before = vm.jit_enabled();

    // Drive a future that errors on compile.
    let _ = block_on(vm.eval_async("not valid lua syntax !@#")).unwrap_err();

    // jit_enabled restored.
    assert_eq!(
        vm.jit_enabled(),
        jit_before,
        "syntax-error early-return must restore jit_enabled"
    );

    // Vm is reusable for both sync and async.
    let r = vm.eval("return 1+1").expect("sync eval works post-err");
    assert!(matches!(r[0], Value::Int(2)));

    let r2 = block_on(vm.eval_async("return 'ok'")).expect("async eval works post-err");
    match &r2[0] {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"ok"),
        other => panic!("expected 'ok', got {other:?}"),
    }
}

/// JIT-disabled-before-future case: enter with `jit_enabled = false`,
/// run an async eval, verify it stays `false` on exit. The
/// `saved_jit_enabled` mechanism's value comes from explicitly
/// covering BOTH the on→off→on and off→off→off restoration paths
/// (existing tests only happen to cover whichever default the
/// sandbox starts with).
#[test]
fn eval_async_preserves_jit_disabled_setting() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_jit_enabled(false);
    assert!(!vm.jit_enabled(), "precondition: jit_enabled = false");

    let r = block_on(vm.eval_async("return 7*6")).expect("eval ok");
    assert!(matches!(r[0], Value::Int(42)));

    assert!(
        !vm.jit_enabled(),
        "jit_enabled stayed false through async eval (saved/restored correctly)"
    );
}

/// Polling an `EvalFuture` after it has returned `Poll::Ready` must
/// panic — `Future` contract requires futures not to be polled
/// after completion (`async_drive.rs:552`: `EvalState::Done =>
/// panic!`).
#[test]
#[should_panic(expected = "polled after Poll::Ready")]
fn eval_future_panics_when_polled_after_ready() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = vm.eval_async("return 1");
    let mut pinned = unsafe { Pin::new_unchecked(&mut fut) };
    // Drive to Ready.
    loop {
        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(_) => break,
            Poll::Pending => continue,
        }
    }
    // One more poll → contract violation, must panic.
    let _ = pinned.as_mut().poll(&mut cx);
}
