//! A table a trace builds and never lets out stays "sunk": its fields
//! live in virtual registers and its own register keeps stale bits until
//! an exit materialises it. Every place where something other than a
//! sunk-aware op can see that register has to get a real table. These
//! shapes reached a stale one:
//!
//! * a local of the enclosing scope set to the iteration's table
//!   (`last = t`, `prev = {n = i}`): at the end of the numeric or generic
//!   `for` the loop was left, or the next iteration read it, with the
//!   table of the iteration the trace was recorded on;
//! * an exit inside the body with the table live in a second register
//!   (`local u = t`), or with a table that has only hash fields: the
//!   exit materialised the array-part tables, into their first register
//!   only;
//! * plain ops reading the register: `t and t[1]`, `m[t] = v`.
//!
//! Each script runs on every dialect and both trace code generators, at
//! several trace thresholds, and must print what the interpreter prints.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const VERSIONS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];
const TIERS: [TraceTier; 2] = [TraceTier::Baseline, TraceTier::Optimizing];

fn show(r: Result<Vec<Value>, String>) -> String {
    match r {
        Ok(v) => match v.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {e}"),
    }
}

/// Runs `src` (which returns a string) with the interpreter and then with
/// the trace JIT on every version, tier and threshold; any difference
/// fails. `traced` lists the versions on which some trace must have run.
fn check(src: &str, traced: &[LuaVersion]) {
    let mut bad = Vec::new();
    for v in VERSIONS {
        let mut vm = luna_jit::new_with_jit(v);
        vm.set_jit_enabled(false);
        vm.set_trace_jit_enabled(false);
        let want = show(vm.eval(src).map_err(|e| vm.error_text(&e)));
        let mut dispatched = 0;
        for tier in TIERS {
            for hot in [None, Some(1), Some(7)] {
                let mut vm = luna_jit::new_with_jit(v);
                vm.set_jit_enabled(false);
                vm.set_trace_jit_enabled(true);
                vm.set_trace_tier(tier);
                if let Some(h) = hot {
                    vm.jit.trace_hot_threshold = h;
                }
                let got = show(vm.eval(src).map_err(|e| vm.error_text(&e)));
                dispatched += vm.trace_dispatched_count();
                if got != want {
                    bad.push(format!("{v:?} {tier:?} hot {hot:?}: {got} (interpreter: {want})"));
                }
            }
        }
        if traced.contains(&v) {
            assert!(dispatched > 0, "{v:?}: no trace ran");
        }
    }
    assert!(bad.is_empty(), "JIT differs from the interpreter:\n{}", bad.join("\n"));
}

/// The versions whose numeric `for` runs as a trace (the trace JIT
/// compiles the loop-count form of 5.4 on).
const NUMERIC: [LuaVersion; 2] = [LuaVersion::Lua54, LuaVersion::Lua55];

#[test]
fn chain_of_tables_through_an_outer_local() {
    check(
        "local last = {n = 0}
         for i = 1, 400 do
           local t = {n = i}
           t.prev = last
           last = t
         end
         local k, cur = 400, last
         while cur.n ~= 0 do
           if cur.n ~= k then return 'broken at ' .. k .. ': ' .. tostring(cur.n) end
           k, cur = k - 1, cur.prev
         end
         return last.n .. ' ' .. k",
        &NUMERIC,
    );
}

#[test]
fn previous_iteration_table_read_in_the_next() {
    check(
        "local out = {}
         local prev = {n = 0}
         for i = 1, 400 do
           local s = 'x' .. i
           out[i - 1] = prev
           prev = {n = i, s = s}
         end
         out[400] = prev
         for i = 0, 400 do
           if type(out[i]) ~= 'table' or out[i].n ~= i then
             return 'out[' .. i .. '] = ' .. tostring(out[i])
           end
         end
         return 'ok ' .. prev.s",
        &NUMERIC,
    );
}

#[test]
fn array_and_hash_tables_left_in_an_outer_local() {
    check(
        "local a = {0}
         for i = 1, 400 do a = {i, i + 1} end
         local h = {k = 0}
         for i = 1, 400 do local t = {}; t.k = i; h = t end
         local e = {}
         for i = 1, 400 do local t = {i}; e = t end
         return a[1] .. ' ' .. a[2] .. ' ' .. h.k .. ' ' .. e[1]",
        &NUMERIC,
    );
}

#[test]
fn outer_local_inside_a_function_called_again() {
    check(
        "local function f(n)
           local last = {n = 0}
           for i = 1, n do last = {n = i} end
           return last.n
         end
         return f(400) .. ' ' .. f(50) .. ' ' .. f(3) .. ' ' .. f(400)",
        &NUMERIC,
    );
}

#[test]
fn inner_loop_table_carried_to_the_outer_loop() {
    check(
        "local last = {n = 0}
         for j = 1, 3 do
           local inner = {n = -1}
           for i = 1, 300 do inner = {n = i * j} end
           last = inner
         end
         return tostring(last.n)",
        &NUMERIC,
    );
}

#[test]
fn two_outer_locals_share_the_table() {
    check(
        "local a, b = {n = 0}, {n = 0}
         for i = 1, 400 do local t = {n = i}; a = t; b = t end
         return a.n .. ' ' .. b.n .. ' ' .. tostring(rawequal(a, b))",
        &NUMERIC,
    );
}

#[test]
fn captured_outer_local() {
    check(
        "local last = {n = 0}
         local function get() return last end
         for i = 1, 400 do last = {n = i} end
         return tostring(get().n)",
        &NUMERIC,
    );
}

#[test]
fn break_out_of_the_loop() {
    check(
        "local last = {n = 0}
         for i = 1, 400 do
           last = {n = i}
           if i == 350 then break end
         end
         return tostring(last.n)",
        &NUMERIC,
    );
}

#[test]
fn exit_with_a_hash_only_table_live() {
    check(
        "local r
         for i = 1, 400 do local t = {n = i}; if i == 350 then r = t end end
         local r2
         for i = 1, 400 do local t = {i}; if i == 350 then r2 = t end end
         return r.n .. ' ' .. r2[1]",
        &NUMERIC,
    );
}

#[test]
fn exit_with_the_table_in_two_registers() {
    check(
        "local r
         for i = 1, 400 do local t = {i}; local u = t; if i == 350 then r = u end end
         local r2, r3
         for i = 1, 400 do
           local t = {i}; local u = t; t = i * 2
           if i == 350 then r2 = t; r3 = u end
         end
         local r4
         for i = 1, 400 do local t = {n = i}; local u = t; if i == 350 then r4 = u end end
         return r[1] .. ' ' .. r2 .. ' ' .. r3[1] .. ' ' .. r4.n",
        &NUMERIC,
    );
}

#[test]
fn plain_ops_reading_the_table() {
    check(
        "local s = 0
         for i = 1, 400 do local t = {i}; local x = t and t[1] or 0; s = s + x end
         local m, c = {}, 0
         for i = 1, 400 do local t = {n = i}; m[t] = i end
         for k, v in pairs(m) do if k.n ~= v then return 'key ' .. tostring(k.n) .. ' ~= ' .. v end; c = c + 1 end
         local fs = {}
         for i = 1, 400 do local t = {i}; fs[i] = function() return t[1] end end
         return s .. ' ' .. c .. ' ' .. fs[1]() .. ' ' .. fs[400]()",
        &NUMERIC,
    );
}

#[test]
fn generic_for_carries_the_table_out() {
    let all = VERSIONS;
    check(
        "local arr = {}
         for i = 1, 400 do arr[i] = i end
         local last = {n = 0}
         for _, v in ipairs(arr) do last = {n = v} end
         local last2 = {n = 0}
         for _, v in pairs(arr) do last2 = {n = v} end
         return tostring(last.n) .. ' ' .. tostring(last2.n)",
        &all,
    );
}

#[test]
fn while_and_repeat_loops() {
    check(
        "local last = {n = 0}
         local i = 0
         while i < 400 do i = i + 1; local t = {n = i}; last = t end
         local last2 = {0}
         local j = 0
         repeat j = j + 1; last2 = {j} until j >= 400
         return last.n .. ' ' .. last2[1]",
        &VERSIONS,
    );
}
