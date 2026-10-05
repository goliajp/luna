//! A trace that runs a whole pass and returns to its head through its own
//! tail resumes at the same pc as a guard that leaves at the head. The two
//! returns carry different register kinds: the tail's must be restored as
//! the tail left them (here `r`, set in the pass), not as the guard's
//! snapshot has them (`r` still nil), which dropped the write.

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

const CASES: [&str; 2] = [
    "local function sh(v) if type(v) ~= 'table' then return tostring(v) end return tostring(v.n) end
     local function ck(n)
       local last, r = {n = 0}, nil
       local m = {}
       local i = 0
       while i < n do i = i + 1
         local t = {n = i, i}
         t.prev = last
         local u = t
         m[u] = i
         if i == 5 then r = u end
         last = u
       end
       return sh(last) .. ' ' .. sh(r)
     end
     return ck(40) .. ' ' .. ck(1) .. ' ' .. ck(40)",
    "local function ck(n)
       local r, i = nil, 0
       while i < n do i = i + 1
         if i == 5 then r = i * 10 end
       end
       return tostring(r)
     end
     return ck(40) .. ' ' .. ck(1) .. ' ' .. ck(40)",
];

fn run(v: LuaVersion, src: &str, jit: Option<(TraceTier, Option<u32>)>) -> String {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    match jit {
        None => vm.set_trace_jit_enabled(false),
        Some((tier, hot)) => {
            vm.set_trace_tier(tier);
            if let Some(h) = hot {
                vm.jit.trace_hot_threshold = h;
                vm.jit.call_hot_threshold = h;
            }
        }
    }
    match vm.eval(src).unwrap().first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{v:?}: not a string: {other:?}"),
    }
}

#[test]
fn a_return_through_the_tail_keeps_the_registers_the_pass_wrote() {
    let tiers = [TraceTier::Baseline, TraceTier::Optimizing, TraceTier::Auto];
    for src in CASES {
        for v in DIALECTS {
            let want = run(v, src, None);
            for tier in tiers {
                for hot in (1..=10).map(Some).chain([None]) {
                    let got = run(v, src, Some((tier, hot)));
                    assert_eq!(got, want, "{v:?} {tier:?} hot {hot:?}\n{src}");
                }
            }
        }
    }
}
