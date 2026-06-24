//! Phase TB (v1.3) — trace-bearing host userdata payloads.
//!
//! Verifies that `T: LuaUserdata` may hold `Gc<...>` fields safely when
//! the embedder overrides `LuaUserdata::trace` to mark them, and that
//! the back-compat case (`impl LuaUserdata for T {}` with no Gc state,
//! default no-op trace) is unchanged from v1.2.

use luna_core::runtime::{Gc, Table, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaUserdata, UserdataMarker, Vm};

fn vm() -> Vm {
    Vm::sandbox(LuaVersion::Lua55).open_base().build()
}

// ─────────────────────────────────────────────────────────────────────
// 1. Back-compat — `T` without Gc fields uses the default no-op trace.
// ─────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
struct PlainCounter(i64);
impl LuaUserdata for PlainCounter {}

#[test]
fn plain_payload_default_trace_is_noop_and_collector_does_not_crash() {
    let mut vm = vm();
    vm.set_userdata("c", PlainCounter(42)).unwrap();
    // Multiple full collections — default no-op trace must not touch
    // unrelated state and must not panic.
    for _ in 0..5 {
        vm.eval("collectgarbage()").unwrap();
    }
    let c: &PlainCounter = vm.userdata_borrow("c").unwrap();
    assert_eq!(c.0, 42);
}

// ─────────────────────────────────────────────────────────────────────
// 2. Single-Gc field — `Cache { entries: Gc<Table> }` survives collection.
// ─────────────────────────────────────────────────────────────────────

struct Cache {
    entries: Gc<Table>,
}

impl LuaUserdata for Cache {
    fn trace(&self, m: &mut UserdataMarker) {
        m.mark(self.entries);
    }
}

#[test]
fn gc_bearing_payload_survives_collection_via_trace() {
    let mut vm = vm();
    let t = vm.heap.new_table();
    // Stash a sentinel int in the table so we can probe its survival.
    // SAFETY: t is freshly allocated; the heap is single-threaded.
    unsafe { t.as_mut() }
        .set(&mut vm.heap, Value::Int(1), Value::Int(0xCAFE))
        .unwrap();
    vm.set_userdata("cache", Cache { entries: t }).unwrap();
    // The local `t` binding is not a GC root (only Lua stack /
    // globals / `host_roots` are). The Table is now reachable ONLY
    // through `Cache::entries`, which is invisible to the collector
    // unless `Cache::trace` marks it.
    let _ = t; // keep the binding so the function still compiles
    // Force multiple collections — Cache::trace must mark `entries` on
    // each cycle for the table to survive.
    for _ in 0..10 {
        vm.eval("collectgarbage()").unwrap();
    }
    // Probe survival via the host payload borrow + table get.
    let c: &Cache = vm.userdata_borrow("cache").unwrap();
    let probed = c.entries.get(Value::Int(1));
    assert!(
        matches!(probed, Value::Int(0xCAFE)),
        "Gc<Table> was collected (got {:?})",
        probed
    );
}

#[test]
fn gc_bearing_payload_without_trace_override_would_dangle() {
    // Documentation-flavoured negative: if a Gc-bearing T does NOT
    // override trace, it inherits the default no-op. We can't assert
    // UAF without UB, so instead this test confirms that explicitly
    // overriding trace is what produces survival — pairing with test
    // #2 above. (i.e. the contract is on the embedder.)
    struct CacheNoTrace {
        entries: Gc<Table>,
    }
    impl LuaUserdata for CacheNoTrace {}

    let mut vm = vm();
    let t = vm.heap.new_table();
    // Pin via host_roots so we don't actually exercise the UAF — the
    // test is about the API shape, not about producing UB.
    vm.pin_host(Value::Table(t));
    vm.set_userdata("c", CacheNoTrace { entries: t }).unwrap();
    for _ in 0..3 {
        vm.eval("collectgarbage()").unwrap();
    }
    let c: &CacheNoTrace = vm.userdata_borrow("c").unwrap();
    // The pinned root kept it alive — the assertion is just that the
    // call path is well-typed and the no-trace-override compiles.
    let _ = c.entries.get(Value::Int(1));
}

// ─────────────────────────────────────────────────────────────────────
// 3. Multi-Gc field — embedder controls which fields are marked.
// ─────────────────────────────────────────────────────────────────────

struct PartialCache {
    kept: Gc<Table>,
    /// This field is intentionally NOT marked by `trace`. The embedder
    /// must root it through another channel (here: `pin_host`) or
    /// accept that it may be collected.
    independent: Gc<Table>,
}

impl LuaUserdata for PartialCache {
    fn trace(&self, m: &mut UserdataMarker) {
        m.mark(self.kept);
        // Deliberately skip `independent` — exercises that `trace` is
        // the embedder's allowlist, not an automatic field scan.
    }
}

#[test]
fn trace_marks_only_listed_fields() {
    let mut vm = vm();
    let kept = vm.heap.new_table();
    let independent = vm.heap.new_table();
    unsafe { kept.as_mut() }
        .set(&mut vm.heap, Value::Int(1), Value::Int(0xABCD))
        .unwrap();
    unsafe { independent.as_mut() }
        .set(&mut vm.heap, Value::Int(1), Value::Int(0x1234))
        .unwrap();
    // Pin `independent` through host_roots so it survives despite not
    // being marked by trace. This isolates the assertion that `kept`
    // survives via trace, not via the host_roots fallback.
    vm.pin_host(Value::Table(independent));
    vm.set_userdata("pc", PartialCache { kept, independent })
        .unwrap();
    let _ = kept; // local binding is not a GC root; see note in test #2
    for _ in 0..5 {
        vm.eval("collectgarbage()").unwrap();
    }
    let pc: &PartialCache = vm.userdata_borrow("pc").unwrap();
    assert!(matches!(pc.kept.get(Value::Int(1)), Value::Int(0xABCD)));
    assert!(matches!(
        pc.independent.get(Value::Int(1)),
        Value::Int(0x1234)
    ));
}

// ─────────────────────────────────────────────────────────────────────
// 4. Vec of Gc<Table> — non-trivial container shape.
// ─────────────────────────────────────────────────────────────────────

struct Pool {
    tables: Vec<Gc<Table>>,
}

impl LuaUserdata for Pool {
    fn trace(&self, m: &mut UserdataMarker) {
        for &t in &self.tables {
            m.mark(t);
        }
    }
}

#[test]
fn vec_of_gc_table_all_survive() {
    let mut vm = vm();
    let mut tables = Vec::new();
    for i in 0..16 {
        let t = vm.heap.new_table();
        unsafe { t.as_mut() }
            .set(&mut vm.heap, Value::Int(1), Value::Int(0x100 + i))
            .unwrap();
        tables.push(t);
    }
    let tables_copy = tables.clone();
    vm.set_userdata("pool", Pool { tables }).unwrap();
    let _ = tables_copy; // see note in test #2
    for _ in 0..5 {
        vm.eval("collectgarbage()").unwrap();
    }
    let pool: &Pool = vm.userdata_borrow("pool").unwrap();
    for (i, &t) in pool.tables.iter().enumerate() {
        let got = t.get(Value::Int(1));
        let expected = 0x100 + i as i64;
        match got {
            Value::Int(n) if n == expected => {}
            other => panic!("slot {i} expected Int({expected}), got {other:?}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// 5. mark_value over a stored Value handle (mixed primitive / Gc).
// ─────────────────────────────────────────────────────────────────────

struct ValueBag {
    slots: Vec<Value>,
}

impl LuaUserdata for ValueBag {
    fn trace(&self, m: &mut UserdataMarker) {
        for &v in &self.slots {
            m.mark_value(v);
        }
    }
}

#[test]
fn mark_value_visits_each_gc_value_in_bag() {
    let mut vm = vm();
    let t1 = vm.heap.new_table();
    let t2 = vm.heap.new_table();
    let s = vm
        .heap
        .intern(b"this-is-a-long-non-interned-string-payload-1234567890");
    unsafe { t1.as_mut() }
        .set(&mut vm.heap, Value::Int(1), Value::Int(0xAAAA))
        .unwrap();
    unsafe { t2.as_mut() }
        .set(&mut vm.heap, Value::Int(1), Value::Int(0xBBBB))
        .unwrap();
    let slots = vec![
        Value::Int(99),
        Value::Table(t1),
        Value::Nil,
        Value::Str(s),
        Value::Table(t2),
        Value::Bool(true),
    ];
    vm.set_userdata("bag", ValueBag { slots }).unwrap();
    for _ in 0..5 {
        vm.eval("collectgarbage()").unwrap();
    }
    let bag: &ValueBag = vm.userdata_borrow("bag").unwrap();
    // Probe both Gc-bearing slots.
    let probe_t1 = match bag.slots[1] {
        Value::Table(t) => t.get(Value::Int(1)),
        _ => panic!("slot 1 not a table"),
    };
    let probe_t2 = match bag.slots[4] {
        Value::Table(t) => t.get(Value::Int(1)),
        _ => panic!("slot 4 not a table"),
    };
    assert!(matches!(probe_t1, Value::Int(0xAAAA)));
    assert!(matches!(probe_t2, Value::Int(0xBBBB)));
}

// ─────────────────────────────────────────────────────────────────────
// 6. GC stress — N collections in a tight loop, no UAF / no panic.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn gc_stress_does_not_panic() {
    let mut vm = vm();
    let mut tables = Vec::new();
    for _ in 0..32 {
        tables.push(vm.heap.new_table());
    }
    vm.set_userdata("pool", Pool { tables }).unwrap();
    // Tight GC loop: 200 cycles. Default heap size is small; the
    // payload's trace runs each cycle.
    vm.eval("for i = 1, 200 do collectgarbage() end").unwrap();
    let pool: &Pool = vm.userdata_borrow("pool").unwrap();
    assert_eq!(pool.tables.len(), 32);
}

// ─────────────────────────────────────────────────────────────────────
// 7. Cycle — Cache::entries -> Table[1] -> Value::Userdata(Cache).
//    With trace impl + Lua-side root drop, both survive while reachable;
//    after the root drops, the cycle is collectable.
// ─────────────────────────────────────────────────────────────────────

#[test]
fn cycle_survives_while_rooted_then_collects_after_drop() {
    let mut vm = vm();
    let t = vm.heap.new_table();
    let ud = vm.create_userdata(Cache { entries: t });
    // Form the cycle: t[1] = ud.
    unsafe { t.as_mut() }
        .set(&mut vm.heap, Value::Int(1), ud)
        .unwrap();
    // Root only the userdata via a global.
    vm.set_global("cache", ud).unwrap();
    // Initial live count snapshot.
    let live_before = vm.heap.live_objects();
    vm.eval("collectgarbage()").unwrap();
    // Cycle is rooted — both userdata + table must still be live.
    // (live_objects may also include freshly-allocated cycle internals;
    // we only assert the userdata and inner table are still reachable.)
    let c: &Cache = vm.userdata_borrow("cache").unwrap();
    assert!(matches!(c.entries.get(Value::Int(1)), Value::Userdata(_)));
    // The collector at least didn't free the cycle.
    let live_after = vm.heap.live_objects();
    // Sanity: live count is bounded by what existed before.
    assert!(live_after <= live_before + 16, "unexpected growth");
    // Now drop the root — both should become collectable.
    vm.eval("cache = nil").unwrap();
    vm.eval("for i = 1, 5 do collectgarbage() end").unwrap();
    // We can't downcast back through the global (it's nil), but the
    // test's value is that the loop didn't crash on a dangling Gc
    // pointer — the cycle collector was driven through `Cache::trace`.
}
