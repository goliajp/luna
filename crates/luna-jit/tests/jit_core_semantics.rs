//! Core-language semantics that must survive JIT compilation.
//!
//! Each case warms a function (or a loop) until the method or trace JIT
//! has compiled it, then feeds it the value that makes PUC raise or take
//! an unusual path. The expected strings are PUC's output for the same
//! chunk (5.3.6 / 5.4.9 / 5.5.1 stock builds); the interpreter already
//! agrees, so a mismatch here is a JIT-only divergence.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

fn run(version: LuaVersion, src: &str) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    let cl = vm.load(src.as_bytes(), b"=c").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string result, got {other:?}"),
    }
}

/// A compiled numeric for must still reject a nil limit, and its trip
/// count is unsigned: a NaN limit with a negative step runs toward
/// minint rather than not at all.
#[test]
fn compiled_numeric_for_checks_limit_and_counts_unsigned() {
    let src = "
        local function g(n) local c = 0 for i = 1, n do c = c + 1 end return c end
        for i = 1, 300 do g(3) end
        local ok, e = pcall(g, nil)
        local ok2, e2 = pcall(function() local x for i = 1, x do end end)
        e = e .. ' ' .. e2
        local function h(a, b, s) local c = 0 for i = a, b, s do c = c + 1 if c > 5 then break end end return c end
        for i = 1, 300 do h(1, 3, 1) end
        return e .. ' | ' .. h(1, 0/0, 1) .. ' ' .. h(1, 0/0, -1)";
    assert_eq!(
        run(LuaVersion::Lua53, src),
        "c:2: 'for' limit must be a number c:5: 'for' limit must be a number | 0 6"
    );
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            run(v, src),
            "c:2: bad 'for' limit (number expected, got nil) \
             c:5: bad 'for' limit (number expected, got nil) | 0 6"
        );
    }
}

/// The method JIT reads a 5.1/5.2 upvalue straight as a float. A nil or a
/// numeric string there must still raise or coerce as the interpreter
/// does, on the first call and after the function is hot.
#[test]
fn compiled_upvalue_arithmetic_checks_the_value() {
    let src = "
        local up = nil
        local function f() return up + 1 end
        local ok, e = pcall(f)
        up = 1
        for i = 1, 300 do f() end
        up = '4'
        local s = f()
        up = {}
        local ok2, e2 = pcall(f)
        return e .. ' | ' .. s .. ' | ' .. e2";
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(
            run(v, src),
            "c:3: attempt to perform arithmetic on upvalue 'up' (a nil value) | 5 | \
             c:3: attempt to perform arithmetic on upvalue 'up' (a table value)"
        );
    }
}
