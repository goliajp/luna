//! v1.1 B10 Stage 1 — cooperative-yield core integration tests.
//!
//! These exercise `Vm::eval_async` end-to-end:
//! - pure-Lua chunk completes in one slice
//! - syntax error surfaces from `Initial` state
//! - runtime error propagates from a mid-script `error('boom')`
//! - long Lua loop fires ≥ 2 `BudgetExhausted` yields and still
//!   completes correctly (the cooperative-yield mechanism is
//!   working as designed)
//! - multi-value return surfaces all results
//! - sync `vm.eval` still works after an async future completes
//!   (no leftover `async_mode` / `host_yield_pending` state)
//!
//! No `tokio` dep — the harness uses a 20-line hand-rolled `block_on`
//! plus a poll counter for the long-loop test. luna-core charter
//! requires zero third-party dependencies (F1).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

// ---------- Hand-rolled executor (no tokio dep) ----------

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

/// Drive `fut` to completion, returning the result. Loops on
/// `Poll::Pending` — the long-loop test relies on the future
/// self-rewaking via `wake_by_ref` to yield control.
fn block_on<F: Future>(mut fut: F) -> F::Output {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    // SAFETY: `fut` is stack-pinned and not moved before drop.
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => continue,
        }
    }
}

/// Like `block_on` but counts how many polls happened before
/// `Poll::Ready`. The first poll is always counted; each `Poll::Pending`
/// observed adds one to the counter via the wake-counting waker.
fn block_on_counting<F: Future>(mut fut: F) -> (F::Output, usize) {
    let counter = Arc::new(AtomicUsize::new(0));

    let waker = {
        let counter = counter.clone();
        make_counting_waker(counter)
    };
    let mut cx = Context::from_waker(&waker);
    let mut fut = unsafe { Pin::new_unchecked(&mut fut) };
    let mut polls = 0usize;
    loop {
        polls += 1;
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return (v, polls),
            Poll::Pending => continue,
        }
    }
}

fn make_counting_waker(counter: Arc<AtomicUsize>) -> Waker {
    // Leak the Arc into a raw pointer; the vtable drops it on drop.
    let data = Arc::into_raw(counter) as *const ();

    unsafe fn clone(data: *const ()) -> RawWaker {
        let arc = unsafe { Arc::<AtomicUsize>::from_raw(data as *const AtomicUsize) };
        let cloned = arc.clone();
        // Don't drop the original; keep both refs live.
        std::mem::forget(arc);
        RawWaker::new(Arc::into_raw(cloned) as *const (), &VT)
    }
    unsafe fn wake(data: *const ()) {
        let arc = unsafe { Arc::<AtomicUsize>::from_raw(data as *const AtomicUsize) };
        arc.fetch_add(1, Ordering::SeqCst);
    }
    unsafe fn wake_by_ref(data: *const ()) {
        let arc = unsafe { Arc::<AtomicUsize>::from_raw(data as *const AtomicUsize) };
        arc.fetch_add(1, Ordering::SeqCst);
        std::mem::forget(arc);
    }
    unsafe fn drop_fn(data: *const ()) {
        drop(unsafe { Arc::<AtomicUsize>::from_raw(data as *const AtomicUsize) });
    }
    static VT: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_fn);

    unsafe { Waker::from_raw(RawWaker::new(data, &VT)) }
}

// ---------- Tests ----------

#[test]
fn eval_async_pure_lua_simple() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let r = block_on(vm.eval_async("return 1 + 2")).unwrap();
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Int(3) => {}
        Value::Float(f) if (f - 3.0).abs() < 1e-9 => {}
        other => panic!("expected 3, got {other:?}"),
    }
}

#[test]
fn eval_async_compile_error() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let err = block_on(vm.eval_async("function function function")).unwrap_err();
    // Compile error path — message is non-empty and classified.
    let msg = vm.error_text(&err);
    assert!(!msg.is_empty(), "expected a syntax-error message, got empty");
    assert_eq!(
        vm.error_kind(),
        luna_core::vm::LuaErrorKind::Syntax,
        "expected Syntax classification, got {:?} (msg: {msg})",
        vm.error_kind()
    );
}

#[test]
fn eval_async_runtime_error() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let err = block_on(vm.eval_async("error('boom')")).unwrap_err();
    let msg = vm.error_text(&err);
    assert!(
        msg.contains("boom"),
        "expected runtime error message to contain 'boom', got: {msg}"
    );
}

#[test]
fn eval_async_long_loop_yields() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    // Small slice so the cooperative-yield mechanism fires several
    // times for a moderate loop.
    vm.set_async_slice(500);
    let (r, polls) =
        block_on_counting(vm.eval_async("local s=0 for i=1,5000 do s=s+i end return s"));
    let vals = r.unwrap();
    assert_eq!(vals.len(), 1);
    match vals[0] {
        // Sum of 1..5000 = 12502500.
        Value::Int(12_502_500) => {}
        Value::Float(f) if (f - 12_502_500.0).abs() < 1e-3 => {}
        other => panic!("expected sum, got {other:?}"),
    }
    // Must have yielded at least twice (i.e. ≥ 3 polls: bootstrap +
    // ≥ 2 BudgetExhausted resumes + terminal).
    assert!(
        polls >= 3,
        "expected ≥ 3 polls (cooperative yield should fire), got {polls}"
    );
}

#[test]
fn eval_async_multi_value_return() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    let r = block_on(vm.eval_async("return 1, 2, 3")).unwrap();
    assert_eq!(r.len(), 3);
    let nums: Vec<i64> = r
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            Value::Float(f) => *f as i64,
            other => panic!("non-numeric: {other:?}"),
        })
        .collect();
    assert_eq!(nums, vec![1, 2, 3]);
}

#[test]
fn eval_async_does_not_break_sync_eval() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    // Run an async eval to completion.
    let _ = block_on(vm.eval_async("return 7")).unwrap();
    // Sync eval should still work — no leaked async_mode /
    // host_yield_pending state.
    let r = vm.eval("return 8 + 9").unwrap();
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Int(17) => {}
        Value::Float(f) if (f - 17.0).abs() < 1e-9 => {}
        other => panic!("expected 17, got {other:?}"),
    }

    // And a second async eval after a sync should also work.
    let r2 = block_on(vm.eval_async("return 'ok'")).unwrap();
    assert_eq!(r2.len(), 1);
    match r2[0] {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"ok"),
        other => panic!("expected 'ok' string, got {other:?}"),
    }
}

#[test]
fn eval_async_slice_size_accessors() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    assert_eq!(vm.async_slice(), 10_000, "default slice size");
    vm.set_async_slice(2_000);
    assert_eq!(vm.async_slice(), 2_000);
    // Non-positive clamps to 1.
    vm.set_async_slice(0);
    assert_eq!(vm.async_slice(), 1);
    vm.set_async_slice(-5);
    assert_eq!(vm.async_slice(), 1);
}
