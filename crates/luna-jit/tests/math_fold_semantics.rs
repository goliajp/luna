//! A `math.*` call the JIT folds into inline code must keep the library's
//! semantics: the result's subtype, the sign of a zero, which operand
//! `max`/`min` return, NaN handling and the rounding of each libm call.
//! Each snippet runs with the JIT on and off; the rendered results must
//! be identical, and they are pinned to what PUC prints.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let r = vm.eval(src).expect("eval");
    let s = match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("snippet must return a string, got {other:?}"),
    };
    (s, vm.trace_dispatched_count())
}

/// Runs `src` both ways and checks both against `expected`.
fn same(version: LuaVersion, src: &str, expected: &str) -> u64 {
    let (interp, _) = run(version, src, false);
    let (jit, dispatched) = run(version, src, true);
    assert_eq!(interp, expected, "interpreter");
    assert_eq!(jit, expected, "JIT");
    dispatched
}

const SHOW: &str = r#"
local function show(v)
  return string.format("%.17g", v) .. ":" .. tostring(math.type(v))
end
"#;

#[test]
fn floor_of_integer_stays_integer_in_a_trace() {
    let src = format!(
        "{SHOW}
        local last
        for i = 1, 20000 do local x = i last = math.floor(x) end
        return show(last)"
    );
    let dispatched = same(LuaVersion::Lua54, &src, "20000:integer");
    assert!(
        dispatched > 0,
        "the integer floor fold should still compile"
    );
}

#[test]
fn floor_of_float_in_a_trace_returns_an_integer() {
    let src = format!(
        "{SHOW}
        local last
        for i = 1, 20000 do local x = i + 0.5 last = math.floor(x) end
        return show(last)"
    );
    same(LuaVersion::Lua54, &src, "20000:integer");
}

#[test]
fn floor_of_integer_stays_integer_in_a_method() {
    let src = format!(
        "{SHOW}
        local function g(x) local y = math.floor(x) return y end
        local r
        for i = 1, 20000 do r = g(i) end
        return show(r)"
    );
    same(LuaVersion::Lua53, &src, "20000:integer");
    same(LuaVersion::Lua54, &src, "20000:integer");
}

#[test]
fn max_returns_the_winning_operand_unconverted() {
    let src = format!(
        "{SHOW}
        local last
        for i = 1, 20000 do local a = i last = math.max(a, 2.5) end
        return show(last)"
    );
    same(LuaVersion::Lua54, &src, "20000:integer");
}

#[test]
fn max_and_min_keep_the_first_of_equal_zeros() {
    let src = format!(
        "{SHOW}
        local hi, lo
        local z = 0.0
        for i = 1, 20000 do hi = math.max(-z, z) lo = math.min(z, -z) end
        return show(hi) .. ' ' .. show(lo)"
    );
    same(LuaVersion::Lua54, &src, "-0:float 0:float");
}

#[test]
fn max_with_nan_first_returns_nan() {
    let src = format!(
        "{SHOW}
        local last
        local nan, one = 0/0, 1.0
        for i = 1, 20000 do last = math.max(nan, one) end
        return show(last)"
    );
    same(LuaVersion::Lua54, &src, "nan:float");
}

#[test]
fn max_of_strings_compares_strings() {
    let src = r#"
        local last
        local a, b = "10", "9"
        for i = 1, 20000 do last = math.max(a, b) end
        return last"#;
    same(LuaVersion::Lua54, src, "9");
}

#[test]
fn sqrt_of_a_numeric_string_converts_it() {
    let src = format!(
        "{SHOW}
        local last
        local s = '16'
        for i = 1, 20000 do last = math.sqrt(s) end
        return show(last)"
    );
    same(LuaVersion::Lua54, &src, "4:float");
}

/// 5.3+ `math.atan(y)` is `atan2(y, 1)`, which libm rounds differently
/// from `atan(y)` for some inputs; this is one on macOS (PUC 5.4 prints
/// ...543, libm `atan` gives ...545). The rounding is the platform
/// libm's, so the JIT is compared with the interpreter, which calls
/// `atan2(y, 1)` as lmathlib.c does.
#[test]
fn atan_rounds_like_atan2() {
    let src = r#"
        local x0 = 0.00012682450675524315
        local last
        for i = 1, 20000 do local x = x0 last = math.atan(x) end
        local function f(y) local r = math.atan(y) return r end
        local last2
        for i = 1, 20000 do last2 = f(x0) end
        return string.format("%.17g %.17g", last, last2)"#;
    let (interp, _) = run(LuaVersion::Lua54, src, false);
    let (jit, _) = run(LuaVersion::Lua54, src, true);
    assert_eq!(jit, interp);
}

/// 5.3+ method chunks fold `floor` / `ceil` of a float (an integer when
/// it fits, otherwise the interpreter reruns the call and returns the
/// float) and `atan` as `atan2(y, 1)` (its last digits are the platform
/// libm's, so that result is compared with the interpreter only). Each
/// function must compile.
#[test]
fn method_folds_rounding_and_atan_on_integer_dialects() {
    let src = format!(
        "{SHOW}
        local function fl(x) local y = math.floor(x) return y end
        local function ce(x) local y = math.ceil(x) return y end
        local function at(x) local y = math.atan(x) return y end
        return show(fl(2.5)) .. ' ' .. show(ce(-2.5)) .. ' ' .. show(fl(-0.0))
          .. ' ' .. show(fl(1e300)) .. ' ' .. show(ce(2^63)) .. ' ' .. show(fl(-2^63))"
    );
    let atan = "local function at(x) local y = math.atan(x) return y end
        return string.format('%.17g', at(0.00012682450675524315))";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        same(
            v,
            &src,
            "2:integer -2:integer 0:integer 1.0000000000000001e+300:float \
             9.2233720368547758e+18:float -9.2233720368547758e+18:integer",
        );
        assert_eq!(run(v, atan, true).0, run(v, atan, false).0, "{v:?} atan");
        let mut vm = luna_jit::new_with_jit(v);
        vm.eval(&src).expect("eval");
        vm.eval(atan).expect("eval");
        assert!(
            luna_jit::jit_backend::cache_entry_count(&vm) >= 3,
            "{v:?}: fl / ce / at should compile"
        );
    }
}
