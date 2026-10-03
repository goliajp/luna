//! Values a trace holds only in its registers stay alive across every helper
//! that can run the collector.
//!
//! A trace keeps the registers it writes in machine registers and writes
//! them back to the Lua stack only when it exits. Some helpers it calls can
//! collect: `Op::Concat` steps the collector after folding, and a generic-for
//! call into a native iterator gives the collector its chance when the native
//! returns (and the native may run Lua code that collects). A table the trace
//! just built, or one it holds in a local it updates, is then referenced from
//! nowhere the collector looks unless the trace passes it as a root.
//!
//! Each script first makes the collector due at every safe point, so an
//! unrooted register is freed on the first such helper call, and the check
//! after the loop sees its memory reused. Every case runs on both trace code
//! generators.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const VERSIONS: [LuaVersion; 2] = [LuaVersion::Lua54, LuaVersion::Lua55];
const TIERS: [TraceTier; 2] = [TraceTier::Baseline, TraceTier::Optimizing];

/// Lua that makes the collector due at every safe point from here on: with
/// the pause at 0 the next cycle is due once the heap holds a megabyte, and
/// the ballast keeps it above that. The loop allocates (outside any trace:
/// `string.rep` is not traced) until a finalizer shows a cycle has run, so
/// the pause has taken effect.
fn gc_every_safe_point(v: LuaVersion) -> String {
    let pause = match v {
        LuaVersion::Lua55 => "collectgarbage('param', 'pause', 0)",
        _ => "collectgarbage('setpause', 0)",
    };
    format!(
        "BALLAST = string.rep('x', 8 * 1024 * 1024)
         {pause}
         local cycles = 0
         setmetatable({{}}, {{__gc = function() cycles = cycles + 1 end}})
         while cycles == 0 do local s = string.rep('y', 10000) end"
    )
}

fn show(r: &[Value]) -> String {
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => format!("{other:?}"),
    }
}

/// Runs `setup` (globals the checks use) with the collector set up as
/// [`gc_every_safe_point`] says, then `body`, which returns "ok", on every
/// version and trace tier; `body` must run as a trace.
fn check(setup: &str, body: &str) {
    for v in VERSIONS {
        for tier in TIERS {
            let mut vm = luna_jit::new_with_jit(v);
            vm.set_trace_tier(tier);
            if let Err(e) = vm.eval(&format!("{}\n{setup}", gc_every_safe_point(v))) {
                panic!("{v:?} {tier:?} setup: {e}");
            }
            let before = vm.trace_dispatched_count();
            match vm.eval(body) {
                Ok(r) => assert_eq!(show(&r), "ok", "{v:?} {tier:?}"),
                Err(e) => panic!("{v:?} {tier:?}: {e}"),
            }
            assert!(
                vm.trace_dispatched_count() > before,
                "{v:?} {tier:?}: the loop never ran as a trace"
            );
        }
    }
}

/// `CHECK_CHAIN(last, n)` walks a chain of `{n = k, prev = ...}` tables
/// from `last` down to the `{n = 0}` it started from.
const CHAIN: &str = "function CHECK_CHAIN(last, total)
       local n, cur = total, last
       while cur.n ~= 0 do
         if cur.n ~= n then error('chain broken at ' .. n .. ': ' .. tostring(cur.n)) end
         n, cur = n - 1, cur.prev
       end
       if n ~= 0 then error('chain ended at ' .. n) end
       return 'ok'
     end";

/// The shape the bug was found with: the constructor's table is held only
/// by the trace while the concat for one of its fields runs.
#[test]
fn concat_keeps_the_table_under_construction() {
    check(
        "",
        "local buckets = {}
         for i = 1, 400 do buckets[i] = {tokens = i, name = 'bucket' .. i} end
         for i = 1, 400 do
           local b = buckets[i]
           if type(b) ~= 'table' or b.tokens ~= i or b.name ~= 'bucket' .. i then
             error('bad ' .. i .. ' ' .. type(b) .. ' ' .. tostring(b and b.tokens))
           end
         end
         return 'ok'",
    );
}

/// The original report: the same chunk loaded and run again and again in one
/// Vm, with the collector at its default pacing.
#[test]
fn concat_repro_reloaded_in_one_vm() {
    let chunk = "local buckets = {}
for i = 1, 200 do buckets[i] = {tokens = i, name = \"bucket\" .. i} end
for i = 1, 200 do
  local b = buckets[i]
  if type(b) ~= \"table\" or b.tokens ~= i then error(\"bad \" .. i .. \" \" .. type(b) .. \" \" .. tostring(b and b.tokens) .. \" \" .. tostring(b and b.name)) end
end
return 0";
    for v in VERSIONS {
        for tier in TIERS {
            let mut vm = luna_jit::new_with_jit(v);
            vm.set_trace_tier(tier);
            vm.set_global("SRC", chunk).unwrap();
            let r = vm
                .eval(
                    "for k = 1, 300 do
                       local ok, e = pcall(load(SRC))
                       if not ok then return k .. ': ' .. tostring(e) end
                     end
                     return 'ok'",
                )
                .unwrap_or_else(|e| panic!("{v:?} {tier:?}: {e}"));
            assert_eq!(show(&r), "ok", "{v:?} {tier:?}");
            assert!(vm.trace_dispatched_count() >= 1, "{v:?} {tier:?}");
        }
    }
}

/// A native iterator: the collector runs when `next` returns to the trace,
/// while the newest table of the chain is only in a register.
#[test]
fn native_iterator_keeps_a_loop_carried_table() {
    check(
        CHAIN,
        "local src = {}
         for i = 1, 400 do src['k' .. i] = i end
         local function chain()
           local last = {n = 0}
           local n = 0
           for _ in next, src do
             n = n + 1
             local t = {n = n}
             t.prev = last
             last = t
           end
           return last, n
         end
         return CHECK_CHAIN(chain())",
    );
}

/// A native iterator that calls back into Lua: the trace for an `ipairs`
/// loop is compiled over a plain array, then the array gets an `__index`
/// that allocates, so at the end of the array the iterator runs Lua code
/// that collects while the newest table of the chain is only in a register.
#[test]
fn metamethod_under_a_native_iterator_keeps_a_loop_carried_table() {
    check(
        &format!(
            "{CHAIN}
             SRC = {{}}
             for i = 1, 200 do SRC[i] = i end
             function CHAIN_OVER(src)
               local last = {{n = 0}}
               local n = 0
               for _, v in ipairs(src) do
                 n = n + 1
                 local t = {{n = v}}
                 t.prev = last
                 last = t
               end
               return last, n
             end
             for _ = 1, 20 do CHECK_CHAIN(CHAIN_OVER(SRC)) end
             setmetatable(SRC, {{__index = function(_, k) local box = {{k}} return nil end}})"
        ),
        "local ok
         for _ = 1, 20 do ok = CHECK_CHAIN(CHAIN_OVER(SRC)) end
         return ok",
    );
}
