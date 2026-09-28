//! v2.1 sort.lua regression — `table.sort` comparator that allocates +
//! triggers `collectgarbage()` previously freed Gc-backed values held in
//! a Rust-local `Vec<Value>` snapshot, producing heap-metadata corruption
//! (`nat_return` index-out-of-bounds with a pointer-sized `len`) only on
//! Linux/Windows (macOS allocator papered over it). Fix:
//! `vm.sort_scratch` parks the working set as a GC root so a callback's
//! `collectgarbage()` can't dangle entries. See
//! `crates/luna-core/src/vm/lib_table.rs::t_sort` +
//! `crates/luna-core/src/vm/exec.rs::Vm::gc_roots`.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Mirrors the failing sort.lua block (Lua 5.5 sort.lua:326-330): a
/// `table.sort` whose comparator rewrites the sorted table via `load`
/// and immediately calls `collectgarbage`. Pre-fix this corrupted
/// `Vm.stack` metadata under glibc / Windows allocators.
#[test]
fn sort_compare_callback_with_collectgarbage_does_not_corrupt() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(1usize << 30));
    let r = vm
        .eval(
            r#"
            AA = {"\xE1lo", "\0first :-)", "alo", "then this one", "45", "and a new"}
            table.sort(AA, function (x, y)
                load(string.format("AA[%q] = ''", x), "")()
                collectgarbage()
                return x < y
            end)
            local seen = 0
            for i, v in ipairs(AA) do seen = seen + 1 end
            _G.AA = nil
            return seen
        "#,
        )
        .expect("compare-with-collectgarbage sort should not crash");
    match r.first() {
        Some(Value::Int(n)) => assert_eq!(*n, 6),
        other => panic!("expected Int(6), got {other:?}"),
    }
}

/// Larger run — 1000 string keys with the same callback shape, to
/// stress the partition-loop pivot re-fetch path beyond the small
/// insertion-sort threshold (PUC auxsort switches at length ≥ 4).
#[test]
fn sort_compare_callback_long_with_gc_pressure() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(1usize << 30));
    let r = vm
        .eval(
            r#"
            local t = {}
            for i = 1, 200 do
                t[i] = string.format("k%04d", 200 - i)
            end
            table.sort(t, function (x, y)
                collectgarbage("step", 1)
                return x < y
            end)
            -- post-sort the comparator's GC pressure must not have
            -- swapped in nils / stale pointers.
            local ok = true
            for i = 1, 200 do
                if type(t[i]) ~= "string" then ok = false break end
                if i > 1 and t[i] < t[i-1] then ok = false break end
            end
            return ok
        "#,
        )
        .expect("long sort with step-GC compare should not crash");
    match r.first() {
        Some(Value::Bool(true)) => {}
        other => panic!("expected Bool(true), got {other:?}"),
    }
}
