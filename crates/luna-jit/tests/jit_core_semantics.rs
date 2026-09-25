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

/// A compiled function whose result is a float converts its integer
/// operands first: `a / b` of two integers, `a + 0.5`.
#[test]
fn compiled_float_arithmetic_converts_integer_operands() {
    let src = "
        local function div(a, b) return a / b end
        local function half(a) return a + 0.5 end
        local s = 0
        for i = 1, 300 do s = s + div(i, 4) + half(i) end
        return s .. ' ' .. div(7, 2) .. ' ' .. half(3) .. ' ' .. div(1, 0)";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(run(v, src), "56587.5 3.5 3.5 inf");
    }
}

/// Integer `//` and `%` in a compiled loop round toward minus infinity,
/// a zero divisor raises the interpreter's error instead of trapping,
/// and shifts of 64 or more give 0 either way.
#[test]
fn compiled_integer_division_and_shifts_follow_lua() {
    let src = "
        local s = 0
        for i = 1, 2000 do local a = i - 1000 s = s + a // 7 + a // -7 + a % 7 + a % -7 end
        local x = 0
        for i = 1, 2000 do local k = i % 140 - 70 x = x ~ (1 << k) ~ (-1 >> k) end
        local function f(a, b) return a // b end
        local ok, e = pcall(function() local t = 0 for i = 1, 2000 do t = t + f(i, 2000 - i) end end)
        local ok2, e2 = pcall(function() local t = 0 for i = 1, 2000 do t = t + 5 % (2000 - i) end end)
        return s .. ' ' .. x .. ' | ' .. e .. ' | ' .. e2";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            run(v, src),
            "-1710 6148914690878603264 | c:6: attempt to divide by zero | \
             c:8: attempt to perform 'n%0'"
        );
    }
}

/// A divisor or shift count the trace knows as a constant is divided or
/// shifted by directly; that constant must be the value the register held
/// before the op, also when the op overwrites it (`x = x % 7`).
#[test]
fn compiled_division_and_shifts_by_constants_follow_lua() {
    let src = "
        local function g() local s = 0 for i = -1000, 1000 do local x = i * 7919 x = x % 7 s = s + x local y = i * 7919 y = y // -3 s = s + y end return s end
        local function h() local s = 0 for i = -1000, 1000 do s = s + (i * 31) // -1 + (i * 31) % -1 + (i << 3) + (i >> 70) + (i >> -2) end return s end
        return g() .. ' ' .. h()";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(run(v, src), "5339 0");
    }
}

/// A table read typed by its use (an addend, an indexed table) is checked:
/// a value of another type late in a compiled loop raises as in the
/// interpreter rather than being read as the expected type's bits.
#[test]
fn compiled_table_reads_check_the_value_type() {
    let src = "
        local ok, e = pcall(load([[local t = {} for i = 1, 2000 do t[i] = i end t[1999] = {}
            local i, s = 1, 0 while i <= 2000 do s = s + t[i] i = i + 1 end return s]], '=w'))
        local ok2, e2 = pcall(load([[local u = {} for i = 1, 2000 do u[i] = {v = i} end u[1999] = 7
            local s = 0 for i = 1, 2000 do s = s + u[i].v end return s]], '=f'))
        local w = {} for i = 1, 2000 do w[i] = i end w[1999] = 2.5
        local s3 = 0 for i = 1, 2000 do s3 = s3 + w[i] end
        return e .. ' | ' .. e2 .. ' | ' .. s3";
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            run(v, src),
            "w:2: attempt to perform arithmetic on a table value (field '?') | \
             f:2: attempt to index a number value (field '?') | 1999003.5"
        );
    }
}
