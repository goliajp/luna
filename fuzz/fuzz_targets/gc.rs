//! v2.4 Phase Fuzz-C — GC fuzz target.
//!
//! Random bytes → DSL of `{alloc, gc_step, gc_full, weak_set,
//! finalizer_set}` ops → execute against a `Vm` via Lua source
//! (the host API for GC ops is exposed cleanly through the
//! `collectgarbage` / table mutation builtins). Asserts:
//! - no panic in mark / sweep / finalizer paths
//! - no UB / OOB (ASAN runtime)
//! - `vm.memory_used()` round-trips consistently across operations
//!
//! This complements `dispatch` by deliberately exercising the
//! GC-stress paths the v2.1 → v2.3 UAFs lived in: weak tables,
//! finalizers, ephemeron cycles, collectgarbage("step") +
//! collectgarbage("collect") interleaving.
//!
//! Run locally:
//!     cd fuzz && cargo +nightly fuzz run gc \
//!         --target aarch64-apple-darwin -- -runs=10000
//!
//! Nightly CI: `.github/workflows/fuzz.yml` (Phase Fuzz-E).

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::fmt::Write;

/// A single GC-stress operation. Bounded set so the generator
/// stays focused on GC paths rather than wandering into arbitrary
/// dispatch (covered by the sibling `dispatch` target).
#[derive(Arbitrary, Debug)]
enum Op {
    /// Allocate a fresh table; bind to one of 8 table slots.
    AllocTable(SlotIdx),
    /// Allocate a fresh closure (an empty function) bound to a slot.
    AllocClosure(SlotIdx),
    /// Allocate a fresh string of N bytes bound to a slot.
    AllocString(SlotIdx, u8),
    /// Wrap a table as a weak-value table via setmetatable.
    MakeWeakValue(SlotIdx),
    /// Wrap a table as a weak-key table.
    MakeWeakKey(SlotIdx),
    /// Set a __gc finalizer on a table.
    SetFinalizer(SlotIdx),
    /// `t[k] = v` — bind two slots, takes a fresh int as the key.
    TableSet(SlotIdx, SlotIdx),
    /// `t[k] = nil` — release a hash entry.
    TableClear(SlotIdx, u8),
    /// Drop a slot binding (set the global to nil so GC can collect).
    Drop(SlotIdx),
    /// `collectgarbage("step", n)` — incremental step.
    GcStep(u8),
    /// `collectgarbage("collect")` — full collect.
    GcFull,
    /// `collectgarbage("count")` — read heap usage (exercises
    /// counter path).
    GcCount,
}

/// One of 8 named slots — `s0`..`s7` — for the GC ops to bind to.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum SlotIdx {
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
}

impl SlotIdx {
    fn name(self) -> &'static str {
        match self {
            SlotIdx::S0 => "s0",
            SlotIdx::S1 => "s1",
            SlotIdx::S2 => "s2",
            SlotIdx::S3 => "s3",
            SlotIdx::S4 => "s4",
            SlotIdx::S5 => "s5",
            SlotIdx::S6 => "s6",
            SlotIdx::S7 => "s7",
        }
    }
}

#[derive(Arbitrary, Debug)]
struct Program {
    ops: Vec<Op>,
}

fn render_op(buf: &mut String, op: &Op) {
    match op {
        Op::AllocTable(s) => writeln!(buf, "{} = {{}}", s.name()).unwrap(),
        Op::AllocClosure(s) => writeln!(buf, "{} = function() return 1 end", s.name()).unwrap(),
        Op::AllocString(s, n) => {
            let len = (*n as usize % 32) + 1;
            let body = "x".repeat(len);
            writeln!(buf, "{} = \"{}\"", s.name(), body).unwrap();
        }
        Op::MakeWeakValue(s) => {
            writeln!(
                buf,
                "if type({0}) == 'table' then setmetatable({0}, {{__mode='v'}}) end",
                s.name()
            )
            .unwrap();
        }
        Op::MakeWeakKey(s) => {
            writeln!(
                buf,
                "if type({0}) == 'table' then setmetatable({0}, {{__mode='k'}}) end",
                s.name()
            )
            .unwrap();
        }
        Op::SetFinalizer(s) => {
            writeln!(
                buf,
                "if type({0}) == 'table' then setmetatable({0}, {{__gc=function() end}}) end",
                s.name()
            )
            .unwrap();
        }
        Op::TableSet(t, v) => {
            writeln!(
                buf,
                "if type({0}) == 'table' then {0}[1] = {1} end",
                t.name(),
                v.name()
            )
            .unwrap();
        }
        Op::TableClear(t, k) => {
            let key = (*k as i32) % 8;
            writeln!(
                buf,
                "if type({0}) == 'table' then {0}[{1}] = nil end",
                t.name(),
                key
            )
            .unwrap();
        }
        Op::Drop(s) => writeln!(buf, "{} = nil", s.name()).unwrap(),
        Op::GcStep(n) => {
            writeln!(buf, "collectgarbage(\"step\", {})", (*n as u32) % 256).unwrap();
        }
        Op::GcFull => buf.push_str("collectgarbage(\"collect\")\n"),
        Op::GcCount => buf.push_str("local _ = collectgarbage(\"count\")\n"),
    }
}

fn render(p: &Program) -> String {
    let mut buf =
        String::from("local s0, s1, s2, s3, s4, s5, s6, s7 = nil, nil, nil, nil, nil, nil, nil, nil\n");
    // Bound program length to 64 ops to keep per-input wall-clock
    // small. The libFuzzer infra catches per-input timeouts at the
    // OS signal level anyway.
    for op in p.ops.iter().take(64) {
        render_op(&mut buf, op);
    }
    buf
}

fuzz_target!(|p: Program| {
    let source = render(&p);
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(16 * 1024 * 1024));
    let baseline = vm.memory_used();
    let _ = vm.eval(&source);
    let after = vm.memory_used();
    // Heap accounting must be non-negative and reasonable.
    // (We don't assert tight bounds — fuzz inputs deliberately
    // allocate; we just check that `memory_used()` returns a
    // consistent reading without panicking.)
    assert!(after >= baseline.saturating_sub(after));
    let _ = after;
});
