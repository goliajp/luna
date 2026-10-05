//! Arithmetic on two 5.1 / 5.2 integers in a trace. Those dialects have
//! only doubles; an integer the VM keeps (`#t`, a string length) stands for
//! the double of its value, so `+ - * %` and negation must give what the
//! doubles give: the exact result rounded once, -0 where IEEE gives it, and
//! nan for a modulo by zero. The trace keeps exact results within ±2^53 and
//! leaves for the interpreter otherwise; each program must give the
//! interpreter's result on every trace tier and the method JIT, and its
//! loop must run in a trace.

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const DOUBLES: [LuaVersion; 2] = [LuaVersion::Lua51, LuaVersion::Lua52];

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

/// Every tier and the method JIT agree with the interpreter on `body` (the
/// inside of a function with `z`, `one`, `two`, `p53` and `p62` in scope:
/// integers kept for 0, 1, 2, 2^53 and 2^62), every trace tier enters a
/// trace, and the interpreter's first result is `want`.
fn check(body: &str, want: &str) {
    let src = format!(
        "local z, one, two = #'', #'x', #'xx'
         local p53 = one for _ = 1, 53 do p53 = p53 * two end
         local p62 = p53 for _ = 1, 9 do p62 = p62 * two end
         return function() {body} end"
    );
    for v in DOUBLES {
        let expect = results(&mut interp(v), &src);
        assert_eq!(expect[0], want, "{v:?} interpreter");
        for (name, tier, at) in TIERS {
            let mut vm = traced(v, tier, at);
            assert_eq!(results(&mut vm, &src), expect, "{v:?} {name}");
            assert!(
                vm.trace_dispatched_count() > 0,
                "{v:?} {name}: no trace ran"
            );
        }
        assert_eq!(
            results(&mut method_jit(v), &src),
            expect,
            "{v:?} method JIT"
        );
    }
}

#[test]
fn sum_of_lengths() {
    check(
        "local t, u, s = {1, 2, 3}, {1, 2}, z
         for i = 1, 500 do s = s + #t + #u end
         return string.format('%.0f', s)",
        "2500",
    );
}

#[test]
fn difference_of_lengths_below_zero() {
    check(
        "local t, s = {1, 2, 3}, z
         for i = 1, 500 do s = s - #t end
         return string.format('%.0f', s)",
        "-1500",
    );
}

#[test]
fn integer_loop_counter_in_a_while_loop() {
    check(
        "local n, i, s = #string.rep('x', 300), z, z
         while i < n do s = s + i i = i + one end
         return string.format('%.0f', s)",
        "44850",
    );
}

#[test]
fn sum_stops_growing_past_two_to_the_53() {
    // 2^53 + 1 is not a double: from 2^53 adding one gives 2^53 again
    check(
        "local s = p53 - 100 * one
         for i = 1, 300 do s = s + one end
         return string.format('%.0f %s', s, tostring(s - p53))",
        "9007199254740992 0",
    );
}

#[test]
fn difference_stops_shrinking_past_minus_two_to_the_53() {
    check(
        "local s = 100 * one - p53
         for i = 1, 300 do s = s - one end
         return string.format('%.0f %s', s, tostring(s + p53))",
        "-9007199254740992 0",
    );
}

#[test]
fn sum_rounds_where_the_machine_would_overflow() {
    // 2^62 + 2^62 overflows a machine integer; the doubles give 2^63
    check(
        "local s
         for i = 1, 300 do s = p62 + p62 + (i - i) end
         return string.format('%.0f', s)",
        "9223372036854775808",
    );
}

#[test]
fn sum_of_large_integers_rounds_to_even() {
    // 2^62 + 3 rounds to 2^62 (the doubles there are 1024 apart)
    check(
        "local s, three = z, #'xxx'
         for i = 1, 300 do s = p62 + three end
         return string.format('%.0f %s', s, tostring(s == p62))",
        "4611686018427387904 true",
    );
}

#[test]
fn product_of_lengths() {
    check(
        "local t, u, s = {1, 2, 3}, {1, 2}, z
         for i = 1, 500 do s = s + #t * #u end
         return string.format('%.0f', s)",
        "3000",
    );
}

#[test]
fn product_past_two_to_the_53_rounds() {
    // 2^53 * 3 + 1 is not a double: the products round, and 2^62 * 2^62
    // is far past any integer
    check(
        "local three, s, q = #'xxx'
         for i = 1, 300 do s = (p53 + one) * three q = p62 * p62 end
         return string.format('%.0f %.0f', s, q)",
        "27021597764222976 21267647932558653966460912964485513216",
    );
}

#[test]
fn zero_times_a_negative_is_negative_zero() {
    check(
        "local m, r = z - one
         for i = 1, 300 do r = z * m end
         return tostring(1 / r)",
        "-inf",
    );
}

#[test]
fn modulo_of_integers() {
    check(
        "local n, i, s, k = #string.rep('x', 300), z, z, #'xxxxxxx'
         while i < n do s = s + i % k + (z - i) % k + i % (z - k) i = i + one end
         return string.format('%.0f', s)",
        "897",
    );
}

#[test]
fn modulo_by_zero_and_past_two_to_the_53() {
    // 2^54 % 3 is 1, but `a - floor(a/b)*b` in doubles rounds the product
    // and gives 0
    check(
        "local r1, r2
         for i = 1, 300 do r1 = one % z r2 = (p53 + p53) % #'xxx' end
         return tostring(r1 ~= r1) .. ' ' .. string.format('%.0f', r2)",
        "true 0",
    );
}

#[test]
fn negation() {
    check(
        "local t, s, r = {1, 2, 3}, z
         for i = 1, 300 do s = s + -#t r = -z end
         return string.format('%.0f', s) .. ' ' .. tostring(1 / r)",
        "-900 -inf",
    );
}
