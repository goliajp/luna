//! v1.3 Phase SS-B — `SendVm` smoke tests.
//!
//! Gated behind `feature = "send"`. Run via
//! `cargo test -p luna-core --features send --test send_vm`.
//!
//! Coverage:
//! - Compile-time `Send` assertion on the type itself.
//! - Basic `eval` round-trip on a `SendVm`.
//! - Move across `std::thread::spawn` boundary (the kevy / async
//!   embed shape).
//! - Concurrent multi-thread access via cloned handles (lock
//!   serializes them; no race, no UB, no panic).
//! - Userdata payload set + read through the lock.
//! - HostRootTicket round-trip (Phase SR types compose with SS-B).

#![cfg(feature = "send")]

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::thread;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaUserdata, SendVm, UserdataMethods};

// ─── Test 1 ────────────────────────────────────────────────────────
// Compile-time: `SendVm: Send`. Failing here means the type-level
// contract broke and SS-B's whole reason-for-being is gone.

#[test]
fn send_vm_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<SendVm>();
}

// ─── Test 2 ────────────────────────────────────────────────────────
// Basic eval: construct, open base/math, run "return 1+2", read
// the Int(3) result.

#[test]
fn send_vm_eval_basic() {
    let vm = SendVm::new(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();

    let r = vm.eval("return 1+2").expect("eval 1+2");
    assert_eq!(r.len(), 1);
    assert!(matches!(r[0], Value::Int(3)));
}

// ─── Test 3 ────────────────────────────────────────────────────────
// Move across thread boundary. No tokio dependency in luna-core, so
// we simulate the `async`-task shape with `std::thread::spawn` +
// `move` closure. The point is the compile-time `Send` bound on
// `thread::spawn`'s closure forces `SendVm: Send` to be load-bearing.

#[test]
fn send_vm_holds_across_thread_move() {
    let vm = SendVm::new(LuaVersion::Lua55);
    vm.open_base();

    let handle = thread::spawn(move || {
        // SendVm was constructed on the parent thread; the move into
        // this closure exercises the `Send` impl. Now run a script
        // on the child thread that mutates Vm state.
        vm.set_global("x", 41_i64).unwrap();
        let r = vm.eval("return x + 1").unwrap();
        assert!(matches!(r[0], Value::Int(42)));
    });
    handle.join().expect("child thread panicked");
}

// ─── Test 4 ────────────────────────────────────────────────────────
// 100 concurrent threads, each cloning the SendVm handle, each
// running a small Lua chunk. The lock serializes them — the test is
// "no race / no UB / no panic", not parallel throughput.

#[test]
fn send_vm_100_concurrent_threads() {
    let vm = SendVm::new(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();
    vm.set_global("counter", 0_i64).unwrap();

    let total_iters = Arc::new(AtomicI64::new(0));

    let mut handles = Vec::with_capacity(100);
    for tid in 0..100 {
        let vm = vm.clone();
        let iters = Arc::clone(&total_iters);
        handles.push(thread::spawn(move || {
            // Each thread runs a tiny arithmetic chunk a few times.
            // The lock guarantees the underlying Vm sees one chunk
            // at a time even though 100 OS threads are pounding the
            // handle.
            for _ in 0..10 {
                let r = vm.eval("return 1+1").expect("eval ok");
                assert!(matches!(r[0], Value::Int(2)));
                iters.fetch_add(1, Ordering::Relaxed);
            }
            // Sanity-check thread identity surfaces in a global mutation.
            let script = format!("counter = counter + {}", tid);
            vm.eval(&script).expect("counter mutate");
        }));
    }
    for h in handles {
        h.join().expect("worker panicked");
    }
    assert_eq!(total_iters.load(Ordering::Relaxed), 1000);

    // Final counter value: 0 + sum(0..100) = 4950.
    let r = vm.eval("return counter").unwrap();
    assert!(matches!(r[0], Value::Int(4950)));
}

// ─── Test 5 ────────────────────────────────────────────────────────
// Userdata API through the lock: register a host `Counter`, mutate
// it from Lua, read it back via `get_global` + a tiny Lua method.

#[derive(Debug)]
struct Counter {
    value: i64,
}

impl LuaUserdata for Counter {
    fn type_name() -> &'static str {
        "Counter"
    }
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_method("get", |_vm, this, ()| Ok::<_, _>(this.value));
        m.add_method_mut("incr", |_vm, this, (by,): (i64,)| {
            this.value += by;
            Ok::<_, _>(())
        });
    }
}

#[test]
fn send_vm_set_userdata_through_lock() {
    let vm = SendVm::new(LuaVersion::Lua55);
    vm.open_base();

    vm.set_userdata("c", Counter { value: 10 }).unwrap();
    vm.eval("c:incr(5); c:incr(2)").unwrap();
    let r = vm.eval("return c:get()").unwrap();
    assert!(matches!(r[0], Value::Int(17)));
}

// ─── Test 6 ────────────────────────────────────────────────────────
// HostRootTicket round-trip: pin a Value, read it back, unpin.
// Confirms the SR types compose with the SS-B lock boundary.

#[test]
fn send_vm_pin_unpin_through_lock() {
    let vm = SendVm::new(LuaVersion::Lua55);

    let t1 = vm.pin_host(Value::Int(123));
    let v = vm.read_host(t1).expect("ticket still live");
    assert!(matches!(v, Value::Int(123)));
    vm.unpin(t1).expect("unpin");
    assert!(vm.read_host(t1).is_none(), "stale after unpin");

    // Re-pin gets a (possibly recycled) slot; the old ticket stays
    // stale.
    let t2 = vm.pin_host(Value::Int(456));
    assert!(vm.read_host(t1).is_none());
    assert!(matches!(vm.read_host(t2), Some(Value::Int(456))));
}

// ─── Test 7 ────────────────────────────────────────────────────────
// Clone-and-share: two handles, two threads, both pinning. The
// underlying Vm sees both pins; both tickets stay live across the
// thread boundary.

#[test]
fn send_vm_pin_across_clones() {
    let vm = SendVm::new(LuaVersion::Lua55);
    let vm2 = vm.clone();

    let t1 = vm.pin_host(Value::Int(1));
    let h = thread::spawn(move || vm2.pin_host(Value::Int(2)));
    let t2 = h.join().expect("worker");

    assert_ne!(t1.idx(), t2.idx());
    assert!(matches!(vm.read_host(t1), Some(Value::Int(1))));
    assert!(matches!(vm.read_host(t2), Some(Value::Int(2))));
}

// ─── Test 8 ────────────────────────────────────────────────────────
// Interp-only guarantee: a SendVm constructed via `SendVm::new`
// must have a NullJitBackend. We can't poke at JitState through the
// public surface, but we can confirm the script that would otherwise
// engage the trace JIT (a tight loop) still produces the right
// answer — which is the only embedder-observable contract anyway.

#[test]
fn send_vm_interp_only_loop_runs_correctly() {
    let vm = SendVm::new(LuaVersion::Lua55);
    vm.open_base();
    vm.open_math();

    let r = vm
        .eval(
            r#"
        local s = 0
        for i = 1, 1000 do s = s + i end
        return s
    "#,
        )
        .expect("sum 1..1000");
    // 1000 * 1001 / 2 == 500500
    assert!(matches!(r[0], Value::Int(500500)));
}
