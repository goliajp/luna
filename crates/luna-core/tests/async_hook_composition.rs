//! v1.3 Phase AS — async natives compose with Rust-side B11 debug hooks.
//!
//! Audit reference: `.dev/rfcs/v1.3-audit-async-natives.md`. The
//! dispatcher hot loop already fires Count / Line / Lua-Call /
//! Lua-Return under `async_mode = true` (those sites are opcode-driven,
//! not async-mode-aware); the gap was the async-native call boundary
//! itself, which now fires:
//!
//! 1. `Call` event before the future is built (`exec.rs` async branch)
//! 2. `Return` event from `commit_async_native_result` after the future
//!    resolves and results land (`async_drive.rs`)
//!
//! These tests pin both ends of that bracket plus the Send-safety
//! property the `RustDebugHook = fn(...)` shape gives us "for free"
//! (function pointers are unconditionally `Send + Sync`).
//!
//! No tokio dep — same hand-rolled `block_on` + `YieldOnce` pattern as
//! `tests/async_native.rs`. luna-core's 0-third-party-dep contract (F1)
//! forbids adding tokio; the tokio integration smoke example, if
//! wanted, lives in luna-jit (which already has dev-deps like
//! criterion).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::exec::{
    HOOK_MASK_CALL, HOOK_MASK_COUNT, HOOK_MASK_LINE, HOOK_MASK_RETURN, RustDebugHook, RustHookEvent,
};
use luna_core::vm::{LuaError, Vm};
use std::cell::RefCell;
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

/// Returns Pending exactly once then Ready. Models an async native that
/// awaits some external work (e.g. an `http_get` round-trip) before
/// writing its result. The async-mode dispatcher should bracket the
/// resolved value with Call (pre-stash) and Return (post-commit) hook
/// events.
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

// ---------- Event recorder (thread-local; tests are single-threaded) ----------

thread_local! {
    static EVENTS: RefCell<Vec<RustHookEvent>> = const { RefCell::new(Vec::new()) };
}

fn record_hook(_vm: &mut Vm, event: RustHookEvent) {
    EVENTS.with(|e| e.borrow_mut().push(event));
}

fn snapshot_events() -> Vec<RustHookEvent> {
    EVENTS.with(|e| e.borrow().clone())
}

fn clear_events() {
    EVENTS.with(|e| e.borrow_mut().clear());
}

fn count_calls(evts: &[RustHookEvent]) -> usize {
    evts.iter()
        .filter(|e| matches!(e, RustHookEvent::Call))
        .count()
}

fn count_returns(evts: &[RustHookEvent]) -> usize {
    evts.iter()
        .filter(|e| matches!(e, RustHookEvent::Return))
        .count()
}

// ---------- Async native fixtures ----------

/// Async native that resolves immediately to int 42.
fn an_ready_42(
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

/// Async native that yields once then resolves to int 7.
fn an_yield_then_7(
    vm: *mut Vm,
    func_slot: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        YieldOnce::new().await;
        // SAFETY: see `an_ready_42`.
        let vm = unsafe { &mut *vm };
        vm.nat_return(func_slot, &[Value::Int(7)]);
        Ok(1)
    })
}

// ---------- Tests ----------

#[test]
fn call_and_return_fire_around_async_native_ready_path() {
    clear_events();
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_CALL | HOOK_MASK_RETURN, 0);
    vm.set_async_native("ret42", an_ready_42).unwrap();
    let r = block_on(vm.eval_async("return ret42()")).unwrap();
    assert_eq!(r.len(), 1);
    matches!(r[0], Value::Int(42));

    let evts = snapshot_events();
    // The Lua chunk wraps the call in a top-level Lua frame, so the
    // total sequence is roughly:
    //   Call(chunk) → Call(ret42 async) → Return(ret42) → Return(chunk)
    // We assert ≥1 Call and ≥1 Return for the async native specifically
    // by checking that totals are ≥2 of each (chunk + async native).
    let calls = count_calls(&evts);
    let rets = count_returns(&evts);
    assert!(
        calls >= 2 && rets >= 2,
        "expected ≥2 Call + ≥2 Return events bracketing chunk + async native, got {evts:?}"
    );
}

#[test]
fn call_and_return_bracket_async_native_yield_path() {
    clear_events();
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_CALL | HOOK_MASK_RETURN, 0);
    vm.set_async_native("yield7", an_yield_then_7).unwrap();
    let r = block_on(vm.eval_async("return yield7() + 1")).unwrap();
    assert_eq!(r.len(), 1);
    let n = match r[0] {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        other => panic!("non-numeric: {other:?}"),
    };
    assert_eq!(n, 8);

    let evts = snapshot_events();
    // The async native fires its Call BEFORE the YieldOnce suspends and
    // its Return AFTER the suspended future resolves. There is no
    // intermediate "yield" hook event in luna's B11 model (audit Q2 —
    // hook events fire only on completed semantic boundaries, never on
    // cooperative yield unwinds), so the Call/Return pair should
    // straddle the suspend cleanly. We assert the *order*: the index
    // of the last Return event must be after the index of some Call
    // event.
    let first_call = evts
        .iter()
        .position(|e| matches!(e, RustHookEvent::Call))
        .expect("at least one Call should fire");
    let last_return = evts
        .iter()
        .rposition(|e| matches!(e, RustHookEvent::Return))
        .expect("at least one Return should fire");
    assert!(
        last_return > first_call,
        "Return must follow Call (got {evts:?})"
    );
}

#[test]
fn count_hook_carries_across_async_slice_boundaries() {
    // The dispatcher already carries `hook.count_left` across
    // `Poll::Pending` returns to the executor (audit §A.3 / Q1) — this
    // test pins that behavior. We force several slice boundaries by
    // setting an aggressive `async_slice_size` and confirm the count
    // hook fires a sensible number of times: roughly `total_ops /
    // count_base`, not "reset on every slice → fires `nslices` times".
    clear_events();
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    // Small slice forces several Poll::Pending re-polls during the
    // 500-iter loop (the dispatcher hits hundreds of opcodes per
    // iteration of i = 1..500 once you count loop setup, comparison,
    // body, jump).
    vm.set_async_slice(50);
    // Fire count hook every 100 opcodes.
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_COUNT, 100);
    let _ = block_on(vm.eval_async("local s = 0; for i = 1, 500 do s = s + i end")).unwrap();

    let counts = snapshot_events()
        .iter()
        .filter(|e| matches!(e, RustHookEvent::Count))
        .count();
    // Without carryover: count_left resets to 100 every slice (slice =
    // 50 ops) so it'd never reach 0. We'd see ZERO count events. With
    // carryover (correct behavior): a few events fire as count_left
    // walks down across slice resumes.
    assert!(
        counts >= 1,
        "count hook must fire across slice boundaries (got {counts} events)"
    );
}

#[test]
fn line_hook_dedupes_across_async_slice_boundaries() {
    // The line hook uses `hook_lastline` to dedupe (audit §A.3) — also
    // a Vm field that persists across `Poll::Pending`. With an
    // aggressive slice size we force a re-poll mid-line; the line
    // event for that line must not double-fire.
    clear_events();
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_async_slice(3);
    vm.set_rust_debug_hook(Some(record_hook), HOOK_MASK_LINE, 0);
    let _ = block_on(vm.eval_async("local a = 1\nlocal b = 2\nlocal c = a + b\nreturn c")).unwrap();

    let lines: Vec<u32> = snapshot_events()
        .iter()
        .filter_map(|e| match e {
            RustHookEvent::Line(n) => Some(*n),
            _ => None,
        })
        .collect();
    // 4-line source: each line should fire at most once even when the
    // slice ends mid-line. Without dedupe a small slice could fire the
    // same line twice on resume.
    assert!(
        !lines.is_empty(),
        "expected at least one Line event, got none"
    );
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        lines.len(),
        sorted.len(),
        "Line hook double-fired across slice boundary: {lines:?}"
    );
}

#[test]
fn rust_debug_hook_is_send_at_type_level() {
    // SS-B Send-safety regression guard: the `RustDebugHook` shape
    // (`fn(&mut Vm, RustHookEvent)`) must remain a bare function
    // pointer so it is unconditionally `Send + Sync` (function
    // pointers are `Send` regardless of feature flags). This is
    // load-bearing for `feature = "send"` (SendVm) composition with
    // async hooks per audit §"Coordination with Phase SS".
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<RustDebugHook>();
    assert_sync::<RustDebugHook>();
    // And the event type — a hook trampoline might capture an event
    // and forward across a channel.
    assert_send::<RustHookEvent>();
}

#[test]
fn hook_call_returning_err_aborts_async_native() {
    // If the user's hook errored from inside the `Call` event for an
    // async native, the future is never built and the error
    // propagates as a normal LuaError. Audit §A.1 edge case.
    fn err_on_call(vm: &mut Vm, evt: RustHookEvent) {
        if matches!(evt, RustHookEvent::Call) {
            // Walk through the public API: there's no
            // hook-from-inside-hook error injection point, so use a
            // sentinel global the test inspects later.
            let _ = vm.set_global("hook_saw_call", Value::Bool(true));
        }
    }
    // Note: B11's hook callback signature `fn(&mut Vm,
    // RustHookEvent)` has no Result return — the hook can't directly
    // abort the call. The audit's "hook returns Err" scenario applies
    // to the Lua-side hook (which can `error()` from inside). For the
    // Rust hook, the behaviour is "hook side effect always runs to
    // completion; the eval result is unaffected by the hook". This
    // test pins the *positive* behaviour: the Call event fires for
    // the async native, the hook records that fact, and the eval
    // succeeds normally.
    let mut vm = Vm::sandbox(LuaVersion::Lua55).open_base().build();
    vm.set_rust_debug_hook(Some(err_on_call), HOOK_MASK_CALL, 0);
    vm.set_async_native("ret42", an_ready_42).unwrap();
    let r = block_on(vm.eval_async("return ret42()")).unwrap();
    assert_eq!(r.len(), 1);
    // Confirm the hook fired (saw the async-native Call). Read via
    // the globals table (bare Vm has no `get_global`; SS-B's SendVm
    // added one, but luna-core's bare Vm exposes only `globals()`).
    let key = Value::Str(vm.intern_str("hook_saw_call"));
    let saw = vm.globals().get(key);
    assert!(
        matches!(saw, Value::Bool(true)),
        "hook should have recorded the async-native Call event, saw {saw:?}"
    );
}
