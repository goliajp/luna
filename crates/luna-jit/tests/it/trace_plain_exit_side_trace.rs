//! A trace started in an inner loop leaves through two plain-pc exits:
//! back into the outer loop's body, or out of the outer loop. A side trace
//! recorded from one of them must not run when the trace takes the other:
//! it resumes the frame where it was recorded, which skipped the outer
//! body (here the reset of the inner counter) on later outer iterations.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

/// Programs returning a count, with the count they must return.
const CASES: [(&str, f64); 3] = [
    (
        "local n = 0
         local f = function()
           for i = 1, 3 do
             local w = 0
             while w < 1 do w = w + 1 n = n + 1 end
           end
         end
         for i = 1, 2000 do f() end
         return n",
        6000.0,
    ),
    (
        "local n = 0
         local f = function()
           for i = 1, 2 do
             local w = 0
             repeat w = w + 1 n = n + 1 until w >= 1
           end
         end
         for i = 1, 2000 do f() end
         return n",
        4000.0,
    ),
    (
        "local n = 0
         local list = {1, 2, 3}
         local f = function()
           for _, v in ipairs(list) do
             local w = 0
             while w < 1 do w = w + 1 n = n + v end
           end
         end
         for i = 1, 2000 do f() end
         return n",
        12000.0,
    ),
];

fn number(v: &Value) -> f64 {
    match *v {
        Value::Int(i) => i as f64,
        Value::Float(f) => f,
        ref o => panic!("not a number: {o:?}"),
    }
}

#[test]
fn a_side_trace_runs_only_on_the_exit_it_was_recorded_from() {
    let tiers = [TraceTier::Baseline, TraceTier::Optimizing, TraceTier::Auto];
    let mut compiled = 0;
    for v in DIALECTS {
        for tier in tiers {
            for hot in (1..=10).map(Some).chain([None]) {
                for (src, want) in CASES {
                    let mut vm = luna_jit::new_with_jit(v);
                    vm.set_trace_tier(tier);
                    if let Some(h) = hot {
                        vm.jit.trace_hot_threshold = h;
                        vm.jit.call_hot_threshold = h;
                    }
                    let r = vm.eval(src).unwrap();
                    assert_eq!(number(&r[0]), want, "{v:?} {tier:?} hot {hot:?}\n{src}");
                    compiled += vm.trace_compiled_count();
                }
            }
        }
    }
    // the runs above went through traces, not only the interpreter
    assert!(compiled > 0);
}
