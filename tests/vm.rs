//! P03 VM semantic corpus, slice 2: expressions, locals, assignments,
//! control flow, numeric for, tables, globals. Assertions go through chunk
//! return values (no stdlib yet).

use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::{Error, Vm};

fn eval(src: &str) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => v,
        Err(Error::Syntax(e)) => panic!("syntax error in {src:?}: {e}"),
        Err(Error::Runtime(e)) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    }
}

fn eval1(src: &str) -> Value {
    let mut v = eval(src);
    assert_eq!(v.len(), 1, "expected 1 result from {src:?}");
    let v = v.pop().unwrap();
    // GC-backed values would dangle once the Vm drops — assert through a
    // helper that keeps the Vm alive instead (e.g. check_str)
    assert!(
        !matches!(v, Value::Str(_) | Value::Table(_) | Value::Closure(_)),
        "eval1 must not return GC values; use a vm-scoped helper"
    );
    v
}

#[track_caller]
fn check_str(src: &str, expect: &[u8]) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = match vm.eval(src) {
        Ok(v) => v,
        Err(Error::Syntax(e)) => panic!("syntax error in {src:?}: {e}"),
        Err(Error::Runtime(e)) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
    };
    assert_eq!(v.len(), 1, "expected 1 result from {src:?}");
    match v[0] {
        Value::Str(s) => assert_eq!(
            s.as_bytes(),
            expect,
            "{src:?} → {:?}",
            String::from_utf8_lossy(s.as_bytes())
        ),
        v => panic!("{src:?} → {v:?}, expected a string"),
    }
}

#[track_caller]
fn check_int(src: &str, expect: i64) {
    let v = eval1(src);
    assert!(
        v.raw_eq(Value::Int(expect)) && matches!(v, Value::Int(_)),
        "{src:?} → {v:?}, expected Int({expect})"
    );
}

#[track_caller]
fn check_float(src: &str, expect: f64) {
    let v = eval1(src);
    match v {
        Value::Float(f) => assert!(
            f == expect || (f.is_nan() && expect.is_nan()),
            "{src:?} → {f}, expected {expect}"
        ),
        v => panic!("{src:?} → {v:?}, expected Float({expect})"),
    }
}

#[track_caller]
fn check_bool(src: &str, expect: bool) {
    let v = eval1(src);
    assert!(
        matches!(v, Value::Bool(b) if b == expect),
        "{src:?} → {v:?}, expected Bool({expect})"
    );
}

#[track_caller]
fn check_error(src: &str, contains: &str) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => panic!("{src:?} unexpectedly returned {v:?}"),
        Err(Error::Runtime(e)) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains(contains),
                "{src:?} error {msg:?} does not contain {contains:?}"
            );
        }
        Err(Error::Syntax(e)) => panic!("{src:?} failed to compile: {e}"),
    }
}

#[test]
fn arithmetic_semantics() {
    check_int("return 1 + 2", 3);
    check_int("return 7 * 6 - 2", 40);
    check_float("return 7 / 2", 3.5);
    check_float("return 2 ^ 10", 1024.0);
    check_int("return 7 // 2", 3);
    check_int("return -7 // 2", -4);
    check_int("return 7 % 3", 1);
    check_int("return -7 % 3", 2);
    check_int("return 7 % -3", -2);
    check_float("return 7.5 % 2", 1.5);
    check_float("return -7.5 % 2", 0.5);
    check_float("return 1 + 0.5", 1.5);
    // integer overflow wraps
    check_int("local a = 9223372036854775807 return a + 1", i64::MIN);
    // unary
    check_int("return -(3)", -3);
    check_float("return -(3.5)", -3.5);
    check_int("local x = 5 return -x", -5);
}

#[test]
fn bitwise_semantics() {
    check_int("return 3 & 5", 1);
    check_int("return 3 | 5", 7);
    check_int("return 3 ~ 5", 6);
    check_int("return ~0", -1);
    check_int("return 1 << 4", 16);
    check_int("return 256 >> 4", 16);
    check_int("return 1 << 64", 0);
    check_int("return 1 << 100", 0);
    check_int("return -1 >> 1", i64::MAX);
    check_int("return 1 << -2", 0);
    check_int("return 16 >> -2", 64);
    // float with integral value converts; fractional errors
    check_int("return 3.0 & 5", 1);
    check_error("return 3.5 & 1", "no integer representation");
    check_error("return {} & 1", "bitwise operation");
}

#[test]
fn division_by_zero() {
    check_error("return 1 // 0", "'n//0'");
    check_error("return 1 % 0", "'n%0'");
    // float division by zero is inf/nan, not an error
    check_float("return 1 / 0", f64::INFINITY);
    check_float("return 1.0 // 0", f64::INFINITY);
    check_float("return 0 / 0", f64::NAN);
}

#[test]
fn comparison_semantics() {
    check_bool("return 1 < 2", true);
    check_bool("return 2 < 1", false);
    check_bool("return 1 <= 1", true);
    check_bool("return 2 > 1", true);
    check_bool("return 1 >= 2", false);
    check_bool("return 1 == 1.0", true);
    check_bool("return 1 ~= 1.0", false);
    check_bool("return 'a' < 'b'", true);
    check_bool("return 'abc' <= 'abc'", true);
    check_bool("return 'abc' < 'abd'", true);
    // exact int/float boundary comparisons (2^63 rounds!)
    check_bool("return 9223372036854775807 < 9223372036854775808.0", true);
    check_bool("return 9223372036854775807.0 <= 9223372036854775807", false);
    check_bool("local nan = 0/0 return nan == nan", false);
    check_bool("local nan = 0/0 return nan < nan", false);
    check_bool("return 1 == '1'", false);
    check_error("return 1 < 'x'", "attempt to compare number with string");
    check_error("return {} < {}", "attempt to compare table with table");
}

#[test]
fn logic_and_truthiness() {
    check_int("return 1 and 2", 2);
    assert!(eval1("return nil and 2").is_nil()); // and yields the lhs itself
    check_int("return nil or 5", 5);
    check_int("return false or 5", 5);
    check_int("return 1 or 2", 1);
    check_bool("return not nil", true);
    check_bool("return not 0", false); // 0 is truthy in Lua
    check_int("return (nil and 1) or (false or 7)", 7);
    // rhs not evaluated on short-circuit (would error)
    check_int("local t = nil return false and t.x or 3", 3);
}

#[test]
fn locals_scoping_and_assignment() {
    check_int("local a = 1 local b = a + 1 return a + b", 3);
    check_int("local a = 1 do local a = 100 end return a", 1);
    check_int("local a, b, c = 1, 2 return (c == nil) and (a + b) or 0", 3);
    check_int("local a, b = 1, 2, 3 return a + b", 3);
    check_int("local a = 1 a = a + 10 return a", 11);
    check_int("local a, b = 1, 2 a, b = b, a return a * 10 + b", 21);
    check_int("x = 5 return x + 1", 6);
    check_bool("return rawunset == nil", true); // unknown global reads nil
    check_int("local t = {} t.x = 1 t.x = t.x + 1 return t.x", 2);
}

#[test]
fn control_flow() {
    check_int("if true then return 1 else return 2 end", 1);
    check_int("if false then return 1 else return 2 end", 2);
    check_int(
        "if nil then return 1 elseif 0 then return 2 else return 3 end",
        2,
    );
    check_int(
        "local n = 0 local i = 1 while i <= 10 do n = n + i i = i + 1 end return n",
        55,
    );
    check_int(
        "local n = 0 while true do n = n + 1 if n >= 5 then break end end return n",
        5,
    );
    check_int("local n = 0 repeat n = n + 1 until n >= 3 return n", 3);
    // repeat scope extends over the condition
    check_int(
        "local n = 0 repeat local done = n >= 2 n = n + 1 until done return n",
        3,
    );
}

#[test]
fn numeric_for_semantics() {
    check_int("local s = 0 for i = 1, 10 do s = s + i end return s", 55);
    check_int(
        "local s = 0 for i = 10, 1, -1 do s = s + i end return s",
        55,
    );
    check_int("local s = 0 for i = 1, 10, 2 do s = s + i end return s", 25);
    check_int("local n = 0 for i = 1, 0 do n = n + 1 end return n", 0);
    check_int("local n = 0 for i = 1, 1 do n = n + 1 end return n", 1);
    // float limit with integer start/step keeps integer control variable
    check_int("local s = 0 for i = 1, 2.5 do s = s + i end return s", 3);
    check_bool("for i = 1, 2.5 do return i == 1 end", true);
    // float loop
    check_float(
        "local s = 0.0 for x = 0.5, 2.0, 0.5 do s = s + x end return s",
        5.0,
    );
    // overflow-proof count: full-range loop would hang if implemented naively
    check_int(
        "local n = 0 for i = 9223372036854775805, 9223372036854775807 do n = n + 1 end return n",
        3,
    );
    check_error("for i = 1, 10, 0 do end", "'for' step is zero");
    check_error(
        "for i = {}, 10 do end",
        "'for' initial value must be a number",
    );
    // break in for
    check_int(
        "local s = 0 for i = 1, 100 do if i > 3 then break end s = s + i end return s",
        6,
    );
}

#[test]
fn tables_and_indexing() {
    check_int("local t = {1, 2, 3} return t[1] + t[2] + t[3]", 6);
    check_int("local t = {x = 10, y = 20} return t.x + t.y", 30);
    check_int("local t = {[2 + 2] = 7} return t[4]", 7);
    check_int("local t = {1, 2; x = 3} return t[1] + t[2] + t.x", 6);
    check_int("local t = {} t[1] = 5 t['k'] = 6 return t[1] + t.k", 11);
    check_int("local t = {{1, 2}, {3, 4}} return t[2][1]", 3);
    check_int("return #'hello'", 5);
    check_int("local t = {1, 2, 3} return #t", 3);
    check_int(
        "local t = {} for i = 1, 100 do t[i] = i * 2 end return t[77]",
        154,
    );
    check_bool("local t = {} return t[1] == nil", true);
    check_bool("local t = {a = 1} return t.b == nil", true);
    check_float("local t = {2.0, [3.0] = 9} return t[1.0] + t[3]", 11.0);
    check_error(
        "local x = nil return x.field",
        "attempt to index a nil value",
    );
    check_error("local x = 5 x.field = 1", "attempt to index a number value");
    check_error("local t = {} t[nil] = 1", "table index is nil");
    check_error("local t = {} t[0/0] = 1", "table index is NaN");
}

#[test]
fn concat_semantics() {
    check_str("return 'a' .. 'b' .. 'c'", b"abc");
    check_str("return 1 .. 2", b"12");
    check_str("return 1.5 .. 'x'", b"1.5x");
    check_str("return 'pi=' .. 3.5 .. '!'", b"pi=3.5!");
    check_error("return {} .. 'x'", "attempt to concatenate a table value");
}

#[test]
fn multiple_returns_fixed() {
    let v = eval("return 1, 2, 3");
    assert_eq!(v.len(), 3);
    assert!(v[0].raw_eq(Value::Int(1)));
    assert!(v[2].raw_eq(Value::Int(3)));
    assert_eq!(eval("return").len(), 0);
    assert_eq!(eval("local a = 1").len(), 0);
}

#[test]
fn globals_via_env() {
    check_int("x = 1 y = 2 return x + y", 3);
    check_int(
        "counter = 0 for i = 1, 5 do counter = counter + 1 end return counter",
        5,
    );
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval("answer = 42").unwrap();
    let v = vm.eval("return answer").unwrap();
    assert!(v[0].raw_eq(Value::Int(42)));
}

#[test]
fn error_positions() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let Err(Error::Runtime(e)) = vm.eval("local x = 1\nlocal y = nil\nreturn y.z") else {
        panic!("expected runtime error")
    };
    let msg = vm.error_text(&e);
    assert!(msg.starts_with("eval:3:"), "position missing: {msg}");
}

// ---- slice 3: functions, closures, varargs, generic for, pcall ----

#[test]
fn functions_and_calls() {
    check_int(
        "local function add(a, b) return a + b end return add(2, 3)",
        5,
    );
    check_int("local f = function(x) return x * 2 end return f(21)", 42);
    check_int("function double(x) return x + x end return double(7)", 14);
    check_int(
        "local function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end \
         return fib(15)",
        610,
    );
    // nested definitions and method syntax
    check_int(
        "local t = {v = 10} function t.get() return 1 end function t:geti() return self.v end \
         return t.get() + t:geti()",
        11,
    );
    check_int(
        "local M = {} M.sub = {} function M.sub:m(x) return x + (self.k or 0) end \
         M.sub.k = 5 return M.sub:m(2)",
        7,
    );
    // missing args are nil, extra args dropped
    check_int(
        "local function f(a, b) return (a or 10) + (b or 20) end return f(1)",
        21,
    );
    check_int("local function f(a) return a end return f(1, 2, 3)", 1);
    check_error("local x = 5 x()", "attempt to call a number value");
}

#[test]
fn closures_and_upvalues() {
    check_int(
        "local function counter() local n = 0 return function() n = n + 1 return n end end \
         local c = counter() c() c() return c()",
        3,
    );
    // two closures share one upvalue
    check_int(
        "local n = 0 local function inc() n = n + 1 end local function get() return n end \
         inc() inc() return get()",
        2,
    );
    // per-iteration capture: each closure sees its own i
    check_int(
        "local fs = {} for i = 1, 3 do fs[i] = function() return i end end \
         return fs[1]() * 100 + fs[2]() * 10 + fs[3]()",
        123,
    );
    // upvalue through two levels
    check_int(
        "local x = 7 local function outer() local function inner() return x end return inner() end \
         return outer()",
        7,
    );
    // assignment through SETUPVAL
    check_int(
        "local x = 1 local function set(v) x = v end set(99) return x",
        99,
    );
    // _ENV as upvalue keeps globals working inside functions
    check_int(
        "g = 5 local function f() g = g + 1 return g end return f()",
        6,
    );
}

#[test]
fn varargs_55_semantics() {
    check_int(
        "local function f(...) local a, b = ... return a + b end return f(3, 4)",
        7,
    );
    check_int(
        "local function f(...) return select('#', ...) end return f(1, nil, 3)",
        3,
    );
    check_int(
        "local function f(...) return ... end return (f(1, 2, 3))",
        1,
    );
    let v = eval("local function f(...) return ... end return f(1, 2, 3)");
    assert_eq!(v.len(), 3);
    // named vararg table: t[i], t.n, read-only binding
    check_int(
        "local function f(...t) return t.n end return f(10, 20, 30)",
        3,
    );
    check_int(
        "local function f(...t) return t[2] end return f(10, 20, 30)",
        20,
    );
    check_int("local function f(...t) return t.n end return f()", 0);
    // ... still works alongside the named table
    check_int(
        "local function f(...t) local a = ... return a + t.n end return f(5, 6)",
        7,
    );
    // vararg in the middle truncates to one value
    check_int(
        "local function f(...) local a, b = (...), 100 return a + b end return f(7, 8)",
        107,
    );
    // chunk varargs exist (main is vararg)
    check_int("local n = select('#', ...) return n", 0);
}

#[test]
fn multret_semantics() {
    check_int(
        "local function two() return 1, 2 end local a, b = two() return a * 10 + b",
        12,
    );
    // call in the middle truncates to 1
    check_int(
        "local function two() return 1, 2 end local a, b, c = two(), 9 \
         return a * 100 + b * 10 + (c or 0)",
        190,
    );
    // call results expand in table constructors and call args
    check_int(
        "local function two() return 1, 2 end local t = {two()} return #t",
        2,
    );
    check_int(
        "local function two() return 1, 2 end local t = {two(), two()} return #t",
        3,
    );
    check_int(
        "local function two() return 1, 2 end local function sum(a, b, c) return a + b + (c or 0) end \
         return sum(two(), 10)",
        11,
    );
    check_int(
        "local function two() return 1, 2 end local function sum(a, b, c) return a + b + (c or 0) end \
         return sum(10, two())",
        13,
    );
    // nested propagation through return
    let v =
        eval("local function two() return 1, 2 end local function f() return two() end return f()");
    assert_eq!(v.len(), 2);
}

#[test]
fn tail_calls_do_not_grow_frames() {
    // a million tail-recursive iterations natively (smaller under miri):
    // would explode without frame reuse
    const N: i64 = if cfg!(miri) { 2_000 } else { 1_000_000 };
    check_int(
        &format!(
            "local function loop(n, acc) if n == 0 then return acc end return loop(n - 1, acc + 1) end return loop({N}, 0)"
        ),
        N,
    );
    // tail method call
    check_int(
        "local t = {} function t:f(n) if n == 0 then return 42 end return self:f(n - 1) end \
         return t:f(10000)",
        42,
    );
}

#[test]
fn generic_for_loops() {
    check_int(
        "local t = {10, 20, 30} local s = 0 for i, v in ipairs(t) do s = s + i + v end return s",
        66,
    );
    check_int(
        "local t = {a = 1, b = 2, c = 3} local s = 0 for k, v in pairs(t) do s = s + v end return s",
        6,
    );
    check_int(
        "local t = {x = 1} local n = 0 for k in pairs(t) do n = n + 1 end return n",
        1,
    );
    // custom closure iterator
    check_int(
        "local function range(n) local i = 0 return function() i = i + 1 if i <= n then return i end end end \
         local s = 0 for v in range(5) do s = s + v end return s",
        15,
    );
    // break inside generic for
    check_int(
        "local s = 0 for i, v in ipairs({5, 6, 7}) do if i == 2 then break end s = s + v end return s",
        5,
    );
    check_error("for x in 5 do end", "attempt to call a number value");
}

#[test]
fn pcall_and_error() {
    check_bool("local ok = pcall(function() return 1 end) return ok", true);
    check_bool(
        "local ok = pcall(function() error('boom') end) return ok",
        false,
    );
    check_str(
        "local _, e = pcall(function() error('boom') end) return e",
        b"eval:1: boom",
    );
    // error with a non-string value: passed through unprefixed
    check_int(
        "local _, e = pcall(function() error({code = 42}) end) return e.code",
        42,
    );
    // error(msg, 0): no position
    check_str(
        "local _, e = pcall(function() error('raw', 0) end) return e",
        b"raw",
    );
    // pcall returns the function's results after true
    check_int(
        "local ok, a, b = pcall(function() return 3, 4 end) return a + b",
        7,
    );
    // nested pcall
    check_bool(
        "local ok = pcall(function() local ok2 = pcall(error) return ok2 end) return ok",
        true,
    );
    // runtime errors are caught too
    check_bool(
        "local ok = pcall(function() local x = nil return x.y end) return ok",
        false,
    );
    // assert message and passthrough
    check_str(
        "local _, e = pcall(function() assert(false, 'msg') end) return e",
        b"eval:1: msg",
    );
    check_int("return assert(42)", 42);
}

#[test]
fn builtin_basics() {
    check_str("return type(nil)", b"nil");
    check_str("return type(1)", b"number");
    check_str("return type('x')", b"string");
    check_str("return type({})", b"table");
    check_str("return type(print)", b"function");
    check_str("return type(function() end)", b"function");
    check_str("return tostring(12)", b"12");
    check_str("return tostring(1.5)", b"1.5");
    check_str("return tostring(nil)", b"nil");
    check_str("return tostring(true)", b"true");
    check_int("return select('#', 1, 2, 3)", 3);
    check_int("return (select(2, 7, 8, 9))", 8);
    check_int("return (select(-1, 7, 8, 9))", 9);
    check_bool("return rawequal('a', 'a')", true);
    check_bool("return rawequal({}, {})", false);
    check_int("return rawlen({1, 2, 3})", 3);
    check_int(
        "local t = setmetatable({}, {}) return rawget(t, 'x') == nil and 1 or 0",
        1,
    );
    check_str("return _VERSION", b"Lua 5.5");
    check_int("_G.zz1 = 8 return zz1", 8);
}

#[test]
fn closures_survive_gc() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval(
        "local n = 0
         counter = function() n = n + 1 return n end",
    )
    .unwrap();
    vm.collect_garbage();
    let v = vm.eval("return counter() + counter()").unwrap();
    assert!(v[0].raw_eq(Value::Int(3)));
    vm.collect_garbage();
    let v = vm.eval("return counter()").unwrap();
    assert!(v[0].raw_eq(Value::Int(3)));
}
