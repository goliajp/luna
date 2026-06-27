//! Regression: PUC `testes/heavy.lua::toomanyidx` — a `for i = 1,
//! math.huge do a[i] = i end` loop under `pcall` must bail with a
//! catchable Lua-level error rather than walking the host allocator
//! off a cliff and SIGSEGV-ing the test runner.
//!
//! Tracked at `.dev/known-bugs/fixed/heavy-lua-sigsegv-under-128mb-loadrep.md`.
//!
//! The PUC shape the gate defends against:
//!
//! ```lua
//! -- testes/heavy.lua::toomanyidx
//! local a = {}
//! local st, msg = pcall(function ()
//!   for i = 1, math.huge do
//!     a[i] = i
//!    end
//! end)
//! print("expected error: ", msg)
//! ```
//!
//! Pre-fix on the ubuntu 7 GB CI runner: the array part doubled past
//! `MAX_ASIZE / 2` to `MAX_ASIZE = 1 << 27` slots; peak transient memory
//! during the final `rehash` (old slab + new slab + temporary
//! `old_pairs` Vec) walked the host allocator off a cliff before the
//! `TableError::Overflow` check could fire. Post-fix: arming
//! `Vm::set_memory_cap` lets the dispatch loop notice between turns,
//! run a full collect (which cannot reclaim the growing array — it is
//! reachable via the captured `a`), and raise the catchable
//! `"memory cap exceeded"` Lua error.
//!
//! `crates/luna-core/tests/official_run.rs` arms a 1 GiB cap when
//! running heavy.lua so the same `toomanyidx` body trips the cap
//! rather than the table's `MAX_ASIZE` ceiling — same outcome (PUC
//! "the loop eventually errors out"), tighter resource budget. This
//! test pins the embedder-side knob the harness leans on.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Pin 1 — a `t[i] = i` grow loop under `pcall` bails with a catchable
/// `"memory cap exceeded"` error when the embedder armed the soft cap,
/// instead of running the host allocator to exhaustion.
#[test]
fn toomanyidx_pcall_trips_memory_cap_cleanly() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let baseline = vm.memory_used();
    // 4 MiB of headroom — small enough that the array part trips long
    // before reaching `MAX_ASIZE`; big enough that the inner `pcall`
    // body runs many full doublings (exercising the same `rehash`
    // path heavy.lua hits in production).
    vm.set_memory_cap(Some(baseline + 4 * 1024 * 1024));
    let v = vm
        .eval(
            "local a = {} \
             local ok, err = pcall(function () \
               for i = 1, 1000000000 do a[i] = i end \
             end) \
             return ok, err",
        )
        .expect("pcall completes; cap fires inside the pcall body");
    assert_eq!(v.len(), 2);
    assert!(
        matches!(v[0], Value::Bool(false)),
        "pcall should catch the cap error, got {:?}",
        v[0]
    );
    match v[1] {
        Value::Str(s) => assert!(
            s.as_bytes().windows(15).any(|w| w == b"memory cap exce"),
            "msg should mention the cap; got {:?}",
            String::from_utf8_lossy(s.as_bytes())
        ),
        v => panic!("expected error string, got {v:?}"),
    }
}

/// Pin 2 — once the cap has fired, it disarms (fire-once contract). A
/// follow-up statement runs without further pressure, mirroring
/// heavy.lua's tail `print "OK"` after `toomanyidx` returns.
#[test]
fn memory_cap_disarms_after_firing_inside_pcall() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let baseline = vm.memory_used();
    vm.set_memory_cap(Some(baseline + 4 * 1024 * 1024));
    let v = vm
        .eval(
            "local a = {} \
             local ok, err = pcall(function () \
               for i = 1, 1000000000 do a[i] = i end \
             end) \
             a = nil \
             local b = {} \
             for i = 1, 1000 do b[i] = i end \
             return ok, #b",
        )
        .expect("post-trip statements run without re-arming");
    assert_eq!(v.len(), 2);
    assert!(matches!(v[0], Value::Bool(false)));
    assert!(
        matches!(v[1], Value::Int(1000)),
        "post-trip allocs should proceed; got {:?}",
        v[1]
    );
}

/// Pin 3 — embedder contract: `set_memory_cap(None)` removes the cap so
/// a downstream stress workload can run unbounded. Symmetric to
/// `set_memory_cap(Some(_))` arming, drives the same code path the
/// official_run harness depends on (set once per file, reset after).
#[test]
fn memory_cap_set_none_disarms_explicitly() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let baseline = vm.memory_used();
    vm.set_memory_cap(Some(baseline + 4 * 1024 * 1024));
    vm.set_memory_cap(None);
    let v = vm
        .eval(
            "local a = {} \
             for i = 1, 100000 do a[i] = i end \
             return #a",
        )
        .expect("uncapped run completes without firing");
    assert_eq!(v.len(), 1);
    assert!(
        matches!(v[0], Value::Int(100000)),
        "expected #a == 100000 after explicit disarm, got {:?}",
        v[0]
    );
}
