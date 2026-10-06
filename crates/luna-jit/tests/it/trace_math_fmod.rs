//! Loops calling `math.fmod` run in traces, in every dialect, with the
//! interpreter's results: integer operands (5.3+ divide integers), float
//! operands, signs, and two NaNs (which of them the result is shows in
//! `tostring`: `nan` or `-nan`). Each program runs under the interpreter,
//! both trace tiers, the move from one to the other, and the method JIT;
//! the results must agree and every trace tier must have entered a trace.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

const TIERS: [(&str, TraceTier, u32); 4] = [
    ("baseline", TraceTier::Baseline, 0),
    ("cranelift", TraceTier::Optimizing, 0),
    ("tier up at once", TraceTier::Auto, 1),
    ("tier up partway", TraceTier::Auto, 40),
];

fn interp(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

fn traced(v: LuaVersion, tier: TraceTier, tier_up_at: u32) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = 8;
    vm.jit.call_hot_threshold = 8;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

fn method_jit(v: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    vm.set_trace_jit_enabled(false);
    vm.jit.call_hot_threshold = 1;
    vm
}

/// `src` returns a function returning a string; its result on each of
/// four calls.
fn results(vm: &mut Vm, src: &str) -> Vec<String> {
    let main = vm.load(src.as_bytes(), b"=t").expect("load");
    let f = match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
        Value::Closure(f) => f,
        ref v => panic!("chunk returned {v:?}"),
    };
    (0..4)
        .map(|_| match vm.call_value(Value::Closure(f), &[]) {
            Ok(v) => match v.first() {
                Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                other => panic!("expected a string, got {other:?}"),
            },
            Err(e) => format!("error: {}", vm.error_display(&e)),
        })
        .collect()
}

/// Every tier and the method JIT agree with the interpreter on the
/// function returned by `src`, and every trace tier enters a trace.
fn check(versions: &[LuaVersion], src: &str) -> Vec<String> {
    versions
        .iter()
        .map(|&v| {
            let want = results(&mut interp(v), src);
            for (name, tier, at) in TIERS {
                let mut vm = traced(v, tier, at);
                assert_eq!(results(&mut vm, src), want, "{v:?} {name}");
                assert!(
                    vm.trace_dispatched_count() > 0,
                    "{v:?} {name}: no trace ran (compiled {}, not dispatchable: {:?}, failed: {:?})",
                    vm.trace_compiled_count(),
                    vm.trace_dispatch_off_reasons(),
                    vm.trace_compile_failed_reasons(),
                );
            }
            assert_eq!(results(&mut method_jit(v), src), want, "{v:?} method JIT");
            want[0].clone()
        })
        .collect()
}

#[test]
fn integer_operands() {
    let r = check(
        &ALL,
        "return function()
            local s = 0
            for i = 1, 300 do s = s + math.fmod(i, 7) + math.fmod(-i, 5) end
            return tostring(s)
        end",
    );
    // sum of i % 7 (903) and of the truncated -(i % 5) (-600)
    assert!(r.iter().all(|s| s == "303" || s == "303.0"), "{r:?}");
}

#[test]
fn float_operands() {
    check(
        &ALL,
        "return function()
            local s = 0
            for i = 1, 300 do s = s + math.fmod(i + 0.25, 3.5) - math.fmod(-i * 1.5, 2) end
            return string.format('%.17g', s)
        end",
    );
}

#[test]
fn infinities_and_zeros() {
    check(
        &ALL,
        "return function()
            local out = {}
            for i = 1, 300 do
                out[1] = math.fmod(i, math.huge)
                out[2] = math.fmod(-0.0, i)
                out[3] = math.fmod(i * 1.0, -math.huge)
            end
            return tostring(out[1]) .. ' ' .. tostring(1 / out[2]) .. ' ' .. tostring(out[3])
        end",
    );
}

#[test]
fn two_nans() {
    // which NaN comes back depends on how fmod is computed (the x87
    // `fprem` PUC's build uses keeps the one with the larger significand)
    check(
        &ALL,
        "return function()
            local a, b = 0/0, -(0/0)
            local r1, r2, r3
            for i = 1, 300 do
                r1 = math.fmod(a, b)
                r2 = math.fmod(b, a)
                r3 = math.fmod(a, i)
            end
            return tostring(r1) .. ' ' .. tostring(r2) .. ' ' .. tostring(r3)
        end",
    );
}

#[test]
fn divisor_minus_one_and_zero() {
    // 5.3+: -1 gives 0 (mininteger % -1 overflows in C) and a zero integer
    // divisor is an error raised once the trace has left; 5.1 / 5.2 divide
    // floats and give -0 and nan
    let r = check(
        &ALL,
        "return function()
            local s, big = 0, math.mininteger or -2^63
            for i = 1, 300 do
                local d = 299 - i
                if d < 0 then d = 0 elseif d > 0 then d = -1 end
                s = s + math.fmod(big + i, d)
            end
            return tostring(s)
        end",
    );
    assert!(r[2].contains("zero"), "{r:?}");
}
