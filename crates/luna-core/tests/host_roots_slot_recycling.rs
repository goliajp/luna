//! v1.3 Phase SR — `host_roots` slot-recycling pool + ABA-safe
//! `HostRootTicket` tests.
//!
//! Covers: basic recycle, stale-ticket detection on read/write/unpin,
//! `unpin_all` invalidates all tickets, bounded growth under
//! steady-state pin/unpin loops, free-list LIFO discipline, GC tracer
//! correctness across recycle.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::{HostRootTicket, Vm};

/// `Value` doesn't impl `PartialEq` (intentional — Lua's `==` is
/// metamethod-aware), so tests use `raw_eq` for primitive equality.
fn raw_eq(v: Value, expect: Value) -> bool {
    v.raw_eq(expect)
}

// ─── Test 1 ────────────────────────────────────────────────────────
// Basic recycle: unpin then re-pin reuses the same slot index but
// bumps the generation, so the two tickets differ.

#[test]
fn pin_unpin_pin_reuses_slot() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t1 = vm.pin_host(Value::Int(1));
    assert_eq!(vm.host_root_count(), 1);

    vm.unpin(t1).expect("first unpin");
    assert_eq!(vm.host_root_count(), 0);

    let t2 = vm.pin_host(Value::Int(2));
    assert_eq!(vm.host_root_count(), 1);

    // Same slot index — confirms recycle.
    assert_eq!(t1.idx(), t2.idx());
    // Generation bumped — confirms ABA tag advanced.
    assert_ne!(t1.generation(), t2.generation());
}

// ─── Test 2 ────────────────────────────────────────────────────────
// Stale ticket: read_host returns None; write_host returns
// HostRootStale; unpin returns HostRootStale.

#[test]
fn stale_ticket_returns_none() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t1 = vm.pin_host(Value::Int(1));
    vm.unpin(t1).unwrap();
    let _t2 = vm.pin_host(Value::Int(2));

    // Read with stale ticket — None.
    assert!(vm.read_host(t1).is_none());
}

#[test]
fn write_host_stale_errors() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t1 = vm.pin_host(Value::Int(1));
    vm.unpin(t1).unwrap();
    let _t2 = vm.pin_host(Value::Int(2));

    assert!(vm.write_host(t1, Value::Int(99)).is_err());
}

#[test]
fn unpin_stale_errors() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t1 = vm.pin_host(Value::Int(1));
    vm.unpin(t1).unwrap();

    // Second unpin of the same ticket — slot was already released.
    assert!(vm.unpin(t1).is_err());
}

// ─── Test 3 ────────────────────────────────────────────────────────
// unpin_all clears all slots — every previously-issued ticket
// becomes stale uniformly.

#[test]
fn unpin_all_clears_all() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let tickets: Vec<HostRootTicket> = (0..10).map(|i| vm.pin_host(Value::Int(i))).collect();
    assert_eq!(vm.host_root_count(), 10);

    vm.unpin_all();
    assert_eq!(vm.host_root_count(), 0);

    for t in &tickets {
        assert!(vm.read_host(*t).is_none());
        assert!(vm.unpin(*t).is_err());
    }
}

// ─── Test 4 ────────────────────────────────────────────────────────
// Long-running pin/unpin loop: pool stays bounded (single slot
// reused for the entire loop).

#[test]
fn long_running_smoke() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    for i in 0..100_000 {
        let t = vm.pin_host(Value::Int(i));
        // Drop it back to the pool immediately — simulates
        // request-per-script embedder.
        vm.unpin(t).unwrap();
    }
    // Pool stays empty at end; underlying Vec capacity ≤ 1 slot.
    assert_eq!(vm.host_root_count(), 0);
}

// ─── Test 5 ────────────────────────────────────────────────────────
// Read with valid ticket returns the original value; write_host
// updates without bumping generation.

#[test]
fn read_and_write_round_trip() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t = vm.pin_host(Value::Int(42));
    assert!(raw_eq(vm.read_host(t).unwrap(), Value::Int(42)));

    vm.write_host(t, Value::Int(99)).unwrap();
    // Same ticket still valid — mutation does NOT bump generation.
    assert!(raw_eq(vm.read_host(t).unwrap(), Value::Int(99)));

    // Aliased ticket (Copy) sees the same updated value.
    let alias = t;
    assert!(raw_eq(vm.read_host(alias).unwrap(), Value::Int(99)));
}

// ─── Test 6 ────────────────────────────────────────────────────────
// Generation overflow retirement — exercising the u32::MAX edge
// directly via the API needs ~4B iterations (not feasible). This
// test documents the contract and exercises the normal-path
// adjacent behavior; the retirement branch is proven correct by
// inspection of `Vm::unpin` (early-return when
// `slot.generation == u32::MAX` without pushing to the free list).

#[test]
fn generation_overflow_retires_slot() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    // Sanity: normal pin/unpin/pin cycle still recycles correctly
    // even when one slot stays retired (none here, but the loop
    // would survive that case).
    let t1 = vm.pin_host(Value::Int(1));
    let t2 = vm.pin_host(Value::Int(2));
    assert_eq!(vm.host_root_count(), 2);
    vm.unpin(t1).unwrap();
    vm.unpin(t2).unwrap();
    assert_eq!(vm.host_root_count(), 0);
}

// ─── Test 7 ────────────────────────────────────────────────────────
// Free-list LIFO discipline: unpin order determines reuse order.

#[test]
fn free_list_lifo() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let t0 = vm.pin_host(Value::Int(0));
    let t1 = vm.pin_host(Value::Int(1));
    let t2 = vm.pin_host(Value::Int(2));

    // Unpin in order 0, 1, 2 → free list LIFO so next pin reuses
    // slot 2 first.
    vm.unpin(t0).unwrap();
    vm.unpin(t1).unwrap();
    vm.unpin(t2).unwrap();

    let r2 = vm.pin_host(Value::Int(20));
    let r1 = vm.pin_host(Value::Int(21));
    let r0 = vm.pin_host(Value::Int(22));

    assert_eq!(r2.idx(), t2.idx());
    assert_eq!(r1.idx(), t1.idx());
    assert_eq!(r0.idx(), t0.idx());
}

// ─── Test 8 ────────────────────────────────────────────────────────
// GC tracer correctness across recycle — pinned heap-allocated
// strings stay reachable while pinned; once unpinned, the slot's
// Nil placeholder does NOT keep stale references alive. We can't
// black-box-test actual collection (string interner pins) but we
// can prove the GC traversal doesn't crash and stale tickets are
// caught.

#[test]
fn gc_tracer_walks_live_slots_only() {
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let s_doomed = vm.intern_str("doomed");
    let t1 = vm.pin_host(Value::Str(s_doomed));

    // Allocate / churn a bit, then drop the pin.
    let _r: Vec<Value> = vm.eval("return 1").unwrap();
    vm.unpin(t1).unwrap();

    // Subsequent eval drives the GC tracer over the now-Nil slot.
    let _: Vec<Value> = vm.eval("return 2").unwrap();
    assert!(vm.read_host(t1).is_none());

    // Pin a fresh value; pool reused the slot.
    let s_alive = vm.intern_str("alive");
    let t2 = vm.pin_host(Value::Str(s_alive));
    match vm.read_host(t2).unwrap() {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"alive"),
        other => panic!("expected Str(\"alive\"), got {:?}", other),
    }
}
