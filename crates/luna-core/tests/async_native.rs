//! v1.1 B10 Stage 2 — async native function integration tests.
//!
//! These exercise `Vm::set_async_native` end-to-end:
//! - async native that returns Ready immediately
//! - async native that returns Pending once then Ready (yields + resumes)
//! - async native that returns Err propagates as a Lua error
//! - async native installed but called from sync `eval` errors out
//! - async native returning multiple values surfaces all of them
//!
//! No `tokio` dep — a hand-rolled `block_on` + `YieldOnce` helper future
//! cover the suspend/resume flow. luna-core charter requires zero
//! third-party deps (F1).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};
use std::future::Future;
use std::pin::Pin;
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

/// Drive `fut` to completion. Re-polls on every `Poll::Pending` (the
/// futures under test self-rewake via `wake_by_ref`, so a busy loop is
/// fine — no real I/O involved).
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

/// A helper future that returns `Poll::Pending` exactly once, re-wakes
/// the host, then returns `Poll::Ready(())`. Models a cooperative
/// yield point inside an async native (e.g. an I/O wait that resolves
/// immediately after the first re-poll).
struct YieldOnce {
    yielded: bool,
}

impl YieldOnce {
    fn new() -> Self {
        YieldOnce { yielded: false }
    }
}

impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

// ---------- Async native fixtures ----------

/// Async native that returns the int 42 immediately (no await).
fn an_return_42(
    vm: *mut Vm,
    func_slot: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        // SAFETY: the dispatcher is suspended; EvalFuture holds the
        // unique &mut Vm borrow for this future's full lifetime.
        let vm = unsafe { &mut *vm };
        vm.nat_return(func_slot, &[Value::Int(42)]);
        Ok(1)
    })
}

/// Async native that yields once (Poll::Pending → Poll::Ready) then
/// returns int 7. Exercises the EvalFuture's pending-future
/// resumption path.
fn an_yield_then_seven(
    vm: *mut Vm,
    func_slot: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        YieldOnce::new().await;
        // SAFETY: see `an_return_42`.
        let vm = unsafe { &mut *vm };
        vm.nat_return(func_slot, &[Value::Int(7)]);
        Ok(1)
    })
}

/// Async native that returns an error after a yield.
fn an_error_after_yield(
    vm: *mut Vm,
    _func_slot: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        YieldOnce::new().await;
        // SAFETY: see `an_return_42`.
        let vm = unsafe { &mut *vm };
        let s = vm.intern_str("kaboom");
        Err(LuaError(Value::Str(s)))
    })
}

/// Async native that returns two values (10, 20). Tests multi-value
/// commit through `finish_results`.
fn an_two_values(
    vm: *mut Vm,
    func_slot: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        // SAFETY: see `an_return_42`.
        let vm = unsafe { &mut *vm };
        vm.nat_return(func_slot, &[Value::Int(10), Value::Int(20)]);
        Ok(2)
    })
}

// ---------- Tests ----------

#[test]
fn async_native_immediate_return() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("ret42", an_return_42).unwrap();
    let r = block_on(vm.eval_async("return ret42()")).unwrap();
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Int(42) => {}
        Value::Float(f) if (f - 42.0).abs() < 1e-9 => {}
        other => panic!("expected 42, got {other:?}"),
    }
}

#[test]
fn async_native_yields_then_resumes() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("yield7", an_yield_then_seven).unwrap();
    let r = block_on(vm.eval_async("return yield7() + 1")).unwrap();
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Int(8) => {}
        Value::Float(f) if (f - 8.0).abs() < 1e-9 => {}
        other => panic!("expected 8, got {other:?}"),
    }
}

#[test]
fn async_native_error_propagates() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("boom", an_error_after_yield).unwrap();
    let err = block_on(vm.eval_async("return boom()")).unwrap_err();
    let msg = vm.error_text(&err);
    assert!(
        msg.contains("kaboom"),
        "expected error to mention 'kaboom', got: {msg}"
    );
}

#[test]
fn async_native_called_in_sync_eval_errors() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("ret42", an_return_42).unwrap();
    // Sync `eval` has `async_mode = false`, so calling an async
    // native must surface a typed error instead of crashing or
    // returning nonsense.
    let err = vm.eval("return ret42()").unwrap_err();
    let msg = vm.error_text(&err);
    assert!(
        msg.contains("async native called in sync context"),
        "expected sync-call error, got: {msg}"
    );
}

#[test]
fn async_native_with_multi_returns() {
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("pair", an_two_values).unwrap();
    let r = block_on(vm.eval_async("local a, b = pair() return a, b")).unwrap();
    assert_eq!(r.len(), 2);
    let nums: Vec<i64> = r
        .iter()
        .map(|v| match v {
            Value::Int(n) => *n,
            Value::Float(f) => *f as i64,
            other => panic!("non-numeric: {other:?}"),
        })
        .collect();
    assert_eq!(nums, vec![10, 20]);
}

#[test]
fn async_native_then_sync_eval_still_works() {
    // Mixed-mode regression guard: after an async-native eval, sync
    // `eval` paths must still work — no leaked async_mode /
    // pending_async_native_* state.
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_native("ret42", an_return_42).unwrap();
    let _ = block_on(vm.eval_async("return ret42()")).unwrap();
    let r = vm.eval("return 1 + 2").unwrap();
    assert_eq!(r.len(), 1);
    match r[0] {
        Value::Int(3) => {}
        Value::Float(f) if (f - 3.0).abs() < 1e-9 => {}
        other => panic!("expected 3, got {other:?}"),
    }
}
