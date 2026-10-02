//! Garbage collection: weak tables, finalizers and incremental steps.

use super::*;

#[test]
fn weak_table_dead_key_does_not_alias_reused_alloc() {
    // PUC `setdeadkey` analogue: when GC sweeps a collectable key out of a
    // weak table the Gc pointer in the node is left dangling, and the
    // freed memory is fair game for the next allocator request. A naïve
    // `find_node` then risks `raw_eq` matching the dangling pointer
    // against a freshly-allocated object whose Gc landed at the same
    // address — gc.lua 5.5 :459-:478 hit that ~12% of the time (the
    // swept B-string's slot chained into A's slot, and the post-sweep
    // `a[k] = nil` clobbered the dead slot's val instead of A's). Hammer
    // the same shape: insert a soon-to-be-swept long string + a live
    // string + a soon-to-be-swept table key, gc, then `a[k] = nil` and
    // verify the live entry actually clears.
    check_int(
        "local rounds = 32 \
         for _ = 1, rounds do \
             local a = setmetatable({}, {__mode = 'kv'}) \
             a[string.rep('a', 2^14)] = 1 \
             a[string.rep('b', 2^14)] = {} \
             a[{}] = 2 \
             collectgarbage() \
             local k = next(a) \
             a[k] = nil \
             collectgarbage() \
             assert(next(a) == nil) \
         end \
         return rounds",
        32,
    );
}

#[test]
fn weak_kv_table_marks_surviving_string_keys() {
    // Lua manual §2.5.4: strings in weak tables are not collected as long
    // as their entry is. PUC `iscleared` implements that by marking the
    // string during the scan; the FAILURE mode is a `__mode='kv'` table
    // with a string key and an alive value whose key string gets swept
    // because nothing else holds it (gc.lua 5.5 :459-:478 was an
    // intermittent ~20% failure before this fix). Hammer the scenario
    // with full GCs so the bug, if regressed, surfaces deterministically.
    check_int(
        "local rounds = 16 \
         for _ = 1, rounds do \
             local a = setmetatable({}, {__mode = 'kv'}) \
             a[string.rep('a', 1024)] = 25 \
             a[string.rep('b', 1024)] = {} \
             a[{}] = 14 \
             collectgarbage() \
             local k, v = next(a) \
             assert(type(k) == 'string' and k:sub(1,1) == 'a' and v == 25, \
                    'expected (\"a*\", 25), got ('..tostring(k)..', '..tostring(v)..')') \
             assert(next(a, k) == nil) \
         end \
         return rounds",
        16,
    );
}

#[test]
fn incremental_gc_step() {
    // an incremental ("step") cycle must terminate and actually free garbage:
    // build a pile, drop it, then sweep with the smallest budget until a cycle
    // completes — the loop must end (true is eventually returned) and the heap
    // must shrink below the pre-drop size.
    check_bool(
        "collectgarbage('incremental') collectgarbage() \
         local a = {} for i = 1, 500 do a[i] = {{}} end \
         local before = collectgarbage('count') \
         a = nil \
         repeat until collectgarbage('step', 1) \
         return collectgarbage('count') < before",
        true,
    );
    // stepsize 0 = a single unbounded step that completes the whole cycle
    // (PUC "stop-the-world"): collectgarbage('step') returns true at once.
    check_bool(
        "collectgarbage('incremental') collectgarbage('param', 'stepsize', 0) \
         return collectgarbage('step')",
        true,
    );
    // generational mode: a "step" is a minor (full atomic) collection, so a
    // weak value created since the previous step is cleared immediately.
    // Regression for gengc.lua:122.
    check_bool(
        "collectgarbage('generational') \
         local t = setmetatable({}, {__mode = 'v'}) \
         t[1] = {10} \
         collectgarbage('step') \
         local r = t[1] == nil \
         collectgarbage('incremental') return r",
        true,
    );
}

#[test]
fn gc_finalizers() {
    // __gc runs when a finalizable object is collected.
    check_bool(
        "local finished = false \
         local u = setmetatable({}, {__gc = function () finished = true end}) \
         u = nil \
         collectgarbage() \
         return finished",
        true,
    );
    // the collector is not reentrant: collectgarbage() inside a finalizer
    // returns fail (nil). Regression for gc.lua:698.
    check_bool(
        "local res = true \
         setmetatable({}, {__gc = function () res = collectgarbage() end}) \
         collectgarbage() \
         return res == nil",
        true,
    );
    // adding __gc to a metatable *after* setmetatable does not register the
    // object for finalization (PUC luaC_checkfinalizer is at setmetatable time).
    check_bool(
        "local ran = false \
         local mt = {} \
         local u = setmetatable({}, mt) \
         mt.__gc = function () ran = true end \
         u = nil \
         collectgarbage() \
         return not ran",
        true,
    );
    // db.lua :915: the finalizer's call frame must be tagged so
    // `debug.getinfo(1).namewhat == "metamethod"` and `.name == "__gc"`
    // (PUC marks ci with CIST_FIN). Without the tag, the test's
    // `repeat local a = {} until name` loop never exits.
    check_str(
        "local n = '' \
         setmetatable({}, {__gc = function () \
           local t = debug.getinfo(1) \
           n = t.namewhat .. ':' .. tostring(t.name) \
         end}) \
         collectgarbage() \
         return n",
        b"metamethod:__gc",
    );
    // Lua 5.5 reference manual §2.5.3: "An object can be marked again for
    // finalization by calling setmetatable with a different metatable, or
    // with the same metatable but with a different __gc field." Aliasing a
    // surviving handle across a finalize lets us re-register the same table
    // and have its `__gc` fire a second time. PUC `udata2finalize` clears
    // FINALIZEDBIT to allow the re-registration; the FIN-only guard on
    // `register_finalizable` mirrors that.
    check_int(
        "local count = 0 \
         local alive \
         local mt = {__gc = function (o) count = count + 1; alive = o end} \
         alive = setmetatable({}, mt) \
         alive = nil \
         collectgarbage() \
         setmetatable(alive, mt) \
         alive = nil \
         collectgarbage() \
         return count",
        2,
    );
}

#[test]
fn warn_on_gc_error_5_4_plus() {
    // PUC 5.4+ `__gc` errors are routed through warn ("warn then continue"),
    // wrapped in `error in __gc metamethod (msg)`. No re-raise; the
    // collectgarbage call succeeds.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval(
        "warn('@on') \
         setmetatable({}, {__gc = function () error('@bang@') end}) \
         collectgarbage()",
    )
    .expect("collectgarbage swallows the __gc error under 5.4+");
    let log = vm.warn_log_take();
    assert_eq!(
        log.len(),
        1,
        "exactly one warn emission expected, got {log:?}"
    );
    let line = String::from_utf8_lossy(&log[0]);
    assert!(
        line.contains("error in __gc metamethod") && line.contains("@bang@"),
        "warn line should mention both the wrapper and the inner error: {line}"
    );
}

#[test]
fn ephemeron_weak_key_tables() {
    // a chain of weak-key entries reachable through a root is fully retained
    // (ephemeron fixpoint: marking a value exposes the next key). gc.lua:336.
    check_int(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local x = nil \
         for i = 1, 50 do local n = {}; a[n] = {k = {x}}; x = n end \
         collectgarbage() \
         local n = x local i = 0 \
         while n do n = a[n].k[1]; i = i + 1 end \
         return i",
        50,
    );
    // once the root is dropped, the whole self-referential weak-key chain is
    // collected — no over-retention. gc.lua:340.
    check_bool(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local x = nil \
         for i = 1, 50 do local n = {}; a[n] = {k = {x}}; x = n end \
         x = nil \
         collectgarbage() \
         return next(a) == nil",
        true,
    );
}

#[test]
fn weak_table_string_keys_survive() {
    // strings are 'values' for weak tables (PUC `iscleared`): a string weak
    // key/value is never cleared and is resurrected by the collection, so the
    // entry survives even when no other reference to the string remains.
    // Regression for gc.lua:250.
    check_str(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local s = 'weakkey-' .. tostring(98765) \
         a[s] = 'kept' \
         s = nil \
         collectgarbage() \
         return a['weakkey-98765']",
        b"kept",
    );
}

#[test]
fn weak_tables_clear_dead_entries() {
    // weak-value table: an entry whose value is otherwise unreachable is
    // cleared by a collection; a still-referenced value survives
    check_int(
        "local kept = {} local w = setmetatable({}, {__mode = 'v'}) \
         w.dead = {} w.live = kept \
         collectgarbage() \
         return (w.dead == nil and w.live == kept) and 1 or 0",
        1,
    );
    // weak-key table: an entry whose key is unreachable is dropped
    check_int(
        "local w = setmetatable({}, {__mode = 'k'}) \
         w[{}] = 1 local k = {} w[k] = 2 \
         collectgarbage() \
         local n = 0 for _ in pairs(w) do n = n + 1 end \
         return n",
        1, // only the entry keyed by the live 'k' remains
    );
    // a non-weak table keeps everything
    check_int(
        "local t = {} t[1] = {} collectgarbage() return t[1] ~= nil and 1 or 0",
        1,
    );
}
