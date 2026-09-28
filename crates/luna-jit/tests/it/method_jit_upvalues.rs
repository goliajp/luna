//! The method JIT reads upvalues in two ways — as the target of a
//! recursive call and as a number — and both must still hold when the
//! upvalue is not what the compiled code expects. Each snippet runs with
//! the JIT on and off and must give PUC's result both ways; a chunk
//! whose upvalue does not fit falls back to the interpreter.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run(version: LuaVersion, src: &str, jit: bool) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {e}"),
    }
}

fn same(version: LuaVersion, src: &str, expected: &str) {
    assert_eq!(
        run(version, src, false),
        expected,
        "{version:?} interpreter"
    );
    assert_eq!(run(version, src, true), expected, "{version:?} JIT");
}

/// `a` calls `b` through an upvalue; the compiled body took every such
/// call for a call to itself.
#[test]
fn forward_declared_locals_call_each_other() {
    let src = r#"
        local a, b, c
        a = function(n) if n == 0 then return 0 end; return b(n - 1) + 1 end
        b = function(n) if n == 0 then return 0 end; return c(n - 1) + 2 end
        c = function(n) if n == 0 then return 0 end; return a(n - 1) + 3 end
        return a(10) .. " " .. b(10) .. " " .. c(10)"#;
    for v in ALL {
        same(v, src, "19 20 21");
    }
}

/// The local a recursive function calls itself through can be
/// reassigned; the call then goes to the new function.
#[test]
fn reassigned_recursive_local_calls_the_new_function() {
    let src = r#"
        local function f(n) if n == 0 then return 0 end return f(n - 1) + 1 end
        local g = f
        local first = g(5)
        f = function(n) return 100 end
        return first .. " " .. g(5)"#;
    for v in ALL {
        same(v, src, "5 101");
    }
}

/// 5.1/5.2 read a numeric upvalue as a float without a check; a table
/// with `__add` or a numeric string is not one.
#[test]
fn non_number_upvalue_in_arithmetic() {
    let add = r#"
        local t = setmetatable({}, {__add = function(a, b) return 40 end})
        local function f() return t + 1 end
        return tostring(f())"#;
    let coerce = r#"
        local k = "10"
        local function h(x) return x + k end
        return tostring(h(1))"#;
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        same(v, add, "40");
        same(v, coerce, "11");
    }
}

/// The report that found the upvalue read: a metamethod that yields,
/// reached from a compiled coroutine body (5.2 can yield there, 5.1
/// cannot).
#[test]
fn yielding_metamethod_on_an_upvalue() {
    let src = r#"
        local t = setmetatable({}, {__add = function(a, b) coroutine.yield("add") return 2 end})
        local co = coroutine.create(function() local r = t + 1; return r end)
        local ok, v = coroutine.resume(co)
        return tostring(ok) .. " " .. tostring(v)"#;
    same(
        LuaVersion::Lua51,
        src,
        "false attempt to yield across metamethod/C-call boundary",
    );
    same(LuaVersion::Lua52, src, "true add");
}

/// A method chunk replaces `math.<fn>(x)` by inline code; it has to see
/// a later assignment to the field.
#[test]
fn reassigned_math_function_in_a_method() {
    let src = r#"
        local function g(x) local y = math.sin(x) return y end
        local s = 0
        for i = 1, 10 do s = s + g(1) end
        math.sin = function() return 100 end
        for i = 1, 10 do s = s + g(1) end
        return string.format("%.6f", s)"#;
    for v in ALL {
        same(v, src, "1008.414710");
    }
}

/// A parameter used in float arithmetic is compiled as a float, and an
/// integer argument was converted to one on entry: returned unchanged it
/// came back as `4.0`. From 5.3 such a call runs in the interpreter.
#[test]
fn integer_argument_to_a_float_parameter_stays_an_integer() {
    let src = r#"
        local function g(x) local y = x * 1.5 return x end
        local function f(x) if x > 0.5 then return x end return 0 end
        return tostring(g(4)) .. " " .. tostring(f(3)) .. " " .. tostring(g(2.5))"#;
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        same(v, src, "4 3 2.5");
    }
}
