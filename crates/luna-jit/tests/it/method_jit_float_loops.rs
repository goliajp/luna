//! Pre-5.3 float numeric loops in the method JIT: each loop must give what
//! the interpreter gives, bit for bit, including values past 2^53, steps
//! that are not dyadic, NaN and infinite bounds, and a body that assigns
//! the loop variable.

use luna_core::__private::VmJitState;
use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const PRE53: [LuaVersion; 2] = [LuaVersion::Lua51, LuaVersion::Lua52];

fn run(version: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(false);
    // compile `f` on its first call
    vm.jit_mut().call_hot_threshold = 1;
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.jit().counters.method_compiled)
}

/// `body` is the loop with its header; it sees the locals `s` (a running
/// sum), `n` (the count) and `last` (the last loop value). The method JIT
/// compiles a loaded chunk that returns one value when it is called, so the
/// loop runs in three chunks, each returning one of the three; the snippet
/// returns them printed exactly.
fn snippet(body: &str) -> String {
    let chunk = |v: &str| format!("local s, n, last = 0, 0, -1 {body} return {v}");
    format!(
        "local function run(src)\n\
           local f = assert((loadstring or load)(src))\n\
           local r\n\
           for _ = 1, 3 do r = f() end\n\
           return r\n\
         end\n\
         return string.format('%.17g %d %.17g', run({:?}), run({:?}), run({:?}))",
        chunk("s"),
        chunk("n"),
        chunk("last"),
    )
}

/// Runs the loop with and without the JIT in 5.1 and 5.2; the results must
/// match. With `compiled`, the JIT run must have compiled the three chunks
/// (the method JIT takes `for` loops whose step is the implicit 1, or
/// another small integer it can read from a `LoadI`; the others run in the
/// interpreter either way, and are here for the equality).
#[track_caller]
fn same(body: &str, compiled: bool) -> String {
    let src = snippet(body);
    let mut out = String::new();
    for v in PRE53 {
        let (interp, _) = run(v, &src, false);
        let (jit, made) = run(v, &src, true);
        assert_eq!(
            jit, interp,
            "{v:?}: JIT differs from the interpreter for\n{body}"
        );
        if compiled {
            assert_eq!(made, 3, "{v:?}: the method JIT did not compile\n{body}");
        }
        out = interp;
    }
    out
}

#[test]
fn half_steps_from_a_half() {
    let r = same(
        "for x = 0.5, 30000.5 do s = s + x n = n + 1 last = x end",
        true,
    );
    assert_eq!(r, "450030000.5 30001 30000.5");
}

#[test]
fn integer_values() {
    same(
        "for x = 1, 100000 do s = s + x n = n + 1 last = x end",
        true,
    );
}

#[test]
fn negative_step() {
    same(
        "for x = 10.5, -3.5, -1 do s = s + x n = n + 1 last = x end",
        false,
    );
    same(
        "for x = 100, -100, -7 do s = s + x n = n + 1 last = x end",
        false,
    );
}

#[test]
fn limit_not_on_a_step() {
    same(
        "for x = 0.5, 10.3 do s = s + x n = n + 1 last = x end",
        true,
    );
    same(
        "for x = -7.75, 7.7 do s = s + x n = n + 1 last = x end",
        true,
    );
    same(
        "for x = 0.25, 100.1, 3 do s = s + x n = n + 1 last = x end",
        false,
    );
    same(
        "for x = -7.75, 7.7, 2 do s = s + x n = n + 1 last = x end",
        false,
    );
}

#[test]
fn empty_ranges() {
    same("for x = 5.5, 1 do s = s + x n = n + 1 last = x end", true);
    same(
        "for x = 1, 5.5, -1 do s = s + x n = n + 1 last = x end",
        false,
    );
}

#[test]
fn values_that_are_not_dyadic() {
    // 0.1 has no exact binary form: the counter must stay off
    same(
        "for x = 0.1, 1000 do s = s + x n = n + 1 last = x end",
        true,
    );
    same("for x = 1/3, 50 do s = s + x n = n + 1 last = x end", false);
    same("for x = 0.3, 50 do s = s + x n = n + 1 last = x end", true);
}

#[test]
fn a_step_that_is_not_an_integer() {
    same(
        "for x = 0, 1, 0.1 do s = s + x n = n + 1 last = x end",
        false,
    );
}

#[test]
fn values_across_2_pow_53() {
    // past 2^53 a float step of 1 rounds back (2^53 + 1 is 2^53): the
    // counter must stay off and the loop must stall as the float loop does
    same(
        "for x = 9007199254740987, 9007199254740997 do s = s + (x - 9007199254740980) n = n + 1 last = x if n == 20 then break end end",
        true,
    );
    // 2^52: integers, but |init| + |limit| + 1 is past 2^51, so the float steps
    same(
        "for x = 4503599627370496, 4503599627370501 do s = s + (x - 4503599627370490) n = n + 1 last = x end",
        true,
    );
    // 2^40 + 0.5: halves, inside the 2^50 the counter allows for them
    same(
        "for x = 1099511627776.5, 1099511627786.5 do s = s + (x - 1099511627776) n = n + 1 last = x end",
        true,
    );
    // 2^50 + 0.5: halves just past it
    same(
        "for x = 1125899906842624.5, 1125899906842634.5 do s = s + (x - 1125899906842624) n = n + 1 last = x end",
        true,
    );
    // the same with steps the method JIT does not take
    same(
        "local b = 2^53 for x = b - 5, b + 6, 2 do s = s + (x - b) n = n + 1 last = x - b end",
        false,
    );
    same(
        "local b = -2^53 for x = b + 6, b - 5, -2 do s = s + (x - b) n = n + 1 last = x - b end",
        false,
    );
}

#[test]
fn nan_and_infinite_bounds() {
    same("for x = 1, 0/0 do s = s + x n = n + 1 last = x end", true);
    same("for x = 0/0, 10 do s = s + 1 n = n + 1 end", true);
    same("for x = 1, -1/0 do s = s + x n = n + 1 last = x end", true);
    same(
        "for x = 1, 1/0 do s = s + x n = n + 1 last = x if n == 1000 then break end end",
        true,
    );
    same(
        "for x = -1/0, 10 do n = n + 1 last = x if n == 3 then break end end",
        true,
    );
}

#[test]
fn zero_and_negative_zero_starts() {
    same("for x = -0.0, 3 do s = s + x n = n + 1 last = x end", true);
    same(
        "for x = 0, -3, -1 do s = s + x n = n + 1 last = x end",
        false,
    );
    same("for x = -0.0, -3 do s = s + x n = n + 1 last = x end", true);
}

#[test]
fn body_assigns_the_loop_variable() {
    // 5.1 / 5.2 semantics: the assignment changes the local, not the count
    same(
        "for x = 0.5, 5.5 do x = x * 10 s = s + x n = n + 1 last = x end",
        true,
    );
}

#[test]
fn huge_values_keep_the_float_steps() {
    // 1e300 + 1 rounds back to 1e300: the float loop would never end, so
    // the body stops it
    same(
        "for x = 1e300, 1e300 + 1 do s = s + 1 n = n + 1 last = x if n == 50 then break end end",
        true,
    );
    same(
        "for x = 2^60, 2^60 + 2^12, 2^10 do s = s + 1 n = n + 1 last = x end",
        false,
    );
    same(
        "for x = 1152921504606846976, 1152921504606846986 do s = s + 1 n = n + 1 last = x if n == 20 then break end end",
        true,
    );
}

/// Loops whose bounds are only known when they run (computed from an outer
/// loop's variable `i`, which is 1, so the method JIT decides then which copy
/// of the inner loop to run).
#[track_caller]
fn same_runtime(a: &str, b: &str) {
    same(
        &format!(
            "for i = 1, 1 do local lo, hi = {a}, {b} \
             for x = lo, hi do s = s + x n = n + 1 last = x end end"
        ),
        true,
    );
}

#[test]
fn bounds_known_only_at_run_time() {
    same_runtime("i - 0.5", "i * 30000 + 0.5");
    same_runtime("i", "i * 100000");
    same_runtime("i - 0.5", "i * 10.3");
    same_runtime("i * 0.1", "i * 1000");
    same_runtime("i * 5.5", "i");
    same_runtime("i", "(i - i) / (i - i)");
    same_runtime("(i - i) / (i - i)", "i * 10");
    same_runtime("(i - i) * -1", "i * 3");
    same_runtime("i * 4503599627370496", "i * 4503599627370501");
    same_runtime("i * 1099511627776.5", "i * 1099511627786.5");
    same_runtime("i * 1125899906842624.5", "i * 1125899906842634.5");
    same_runtime("i * 9007199254740987", "i * 9007199254740990");
}
