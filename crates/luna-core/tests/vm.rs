//! P03 VM semantic corpus, slice 2: expressions, locals, assignments,
//! control flow, numeric for, tables, globals. Assertions go through chunk
//! return values (no stdlib yet).

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn eval(src: &str) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
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
        Err(e) => panic!("runtime error in {src:?}: {}", vm.error_text(&e)),
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
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains(contains),
                "{src:?} error {msg:?} does not contain {contains:?}"
            );
        }
    }
}

#[test]
fn arithmetic_semantics() {
    check_int("return 1 + 2", 3);
    check_int("return 7 * 6 - 2", 40);
    check_float("return 7 / 2", 3.5);
    if !cfg!(miri) {
        // miri perturbs powf; exactness is asserted natively only
        check_float("return 2 ^ 10", 1024.0);
    }
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
    check_error("return 1 // 0", "attempt to divide by zero");
    check_error("return 1 % 0", "'n%0'");
    // float division by zero is inf/nan, not an error
    check_float("return 1 / 0", f64::INFINITY);
    check_float("return 1.0 // 0", f64::INFINITY);
    check_float("return 0 / 0", f64::NAN);
}

#[test]
fn concat_result_as_binop_operand() {
    // A CONCAT result is a temporary at the top of the stack; the enclosing
    // binary op must not let the right operand reuse and clobber its register.
    check_int("return (1 .. 2) << 1", 24); // "12" -> 12, 12<<1
    check_int("return (1 .. 2) + 1", 13);
    check_int("return (\"7\" .. 3) << 1", 146);
    check_bool("return \"a\" .. \"b\" > \"a\"", true);
    check_bool("return not(2+1 > 3*1) and \"a\"..\"b\" > \"a\"", true);
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
    check_error("return {} < {}", "attempt to compare two table values");
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
        "bad 'for' initial value (number expected, got table)",
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
    let Err(e) = vm.eval("local x = 1\nlocal y = nil\nreturn y.z") else {
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
    // writing the named table feeds back into `...` (they share storage)
    check_int(
        "local function f(...t) t[1] = t[1] + 10 return (...) end return f(5)",
        15,
    );
    // setting t.n changes how many values `...` expands to
    check_int(
        "local function f(...t) t.n = 3 return select('#', ...) end return f(1)",
        3,
    );
    // an out-of-range t.n is rejected when `...` expands (PUC getnumargs)
    check_error(
        "local function f(...t) t.n = -1 return ... end return f(1)",
        "no proper 'n'",
    );
    check_error(
        "local function f(...t) t.n = 1.0 return ... end return f(1)",
        "no proper 'n'",
    );
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

// ---- slice 4: metamethods ----

#[test]
fn mm_index_and_newindex() {
    // __index table chain (inheritance)
    check_int(
        "local Base = {greet = 4} local Mid = setmetatable({x = 2}, {__index = Base}) \
         local obj = setmetatable({}, {__index = Mid}) return obj.greet + obj.x",
        6,
    );
    // __index function
    check_int(
        "local t = setmetatable({}, {__index = function(t, k) return #k end}) return t.abc",
        3,
    );
    // raw hit short-circuits the chain
    check_int(
        "local t = setmetatable({v = 1}, {__index = function() return 99 end}) return t.v",
        1,
    );
    // __newindex function redirects writes
    check_int(
        "local log = {} local t = setmetatable({}, {__newindex = function(t, k, v) log[k] = v * 2 end}) \
         t.a = 21 return log.a",
        42,
    );
    // __newindex table redirects writes (and reads stay separate)
    check_int(
        "local store = {} local t = setmetatable({}, {__newindex = store}) t.k = 7 \
         return store.k + (rawget(t, 'k') == nil and 1 or 0)",
        8,
    );
    // assignment to an existing key ignores __newindex
    check_int(
        "local n = 0 local t = setmetatable({k = 1}, {__newindex = function() n = 99 end}) \
         t.k = 2 return t.k + n",
        2,
    );
    // chain loop detection
    check_error(
        "local t = {} setmetatable(t, {__index = t}) return t.x",
        "chain too long",
    );
}

#[test]
fn mm_arithmetic_and_string_coercion() {
    check_int(
        "local function val(x) return type(x) == 'table' and x.v or x end \
         local V = {} V.__add = function(a, b) return val(a) + val(b) end \
         local x = setmetatable({v = 40}, V) return x + 2",
        42,
    );
    // right operand's metamethod is found too
    check_int(
        "local V = {__sub = function(a, b) return 7 end} \
         local x = setmetatable({}, V) return 1 - x",
        7,
    );
    check_int(
        "local V = {__unm = function(x) return 5 end} return -setmetatable({}, V)",
        5,
    );
    // string arithmetic coercion (5.5 default)
    check_int("return '10' + 1", 11);
    check_float("return '2.5' * 2", 5.0);
    check_int("return '0x10' + 0", 16);
    check_int("return '8' // '3'", 2);
    check_int("return '12' & 4", 4);
    check_error(
        "return 'abc' + 1",
        "attempt to perform arithmetic on a string value",
    );
}

#[test]
fn mm_comparison() {
    let v = "local V = {__eq = function(a, b) return a.id == b.id end, \
              __lt = function(a, b) return a.id < b.id end, \
              __le = function(a, b) return a.id <= b.id end} \
              local a = setmetatable({id = 1}, V) local b = setmetatable({id = 1}, V) \
              local c = setmetatable({id = 2}, V) ";
    check_bool(&format!("{v} return a == b"), true);
    check_bool(&format!("{v} return a ~= b"), false);
    check_bool(&format!("{v} return a == c"), false);
    check_bool(&format!("{v} return a < c"), true);
    check_bool(&format!("{v} return c <= a"), false);
    check_bool(&format!("{v} return a > c"), false);
    // __eq only fires between tables, never table vs other types
    check_bool(
        "local t = setmetatable({}, {__eq = function() return true end}) return t == 1",
        false,
    );
}

#[test]
fn mm_call_concat_len_tostring() {
    check_int(
        "local t = setmetatable({base = 40}, {__call = function(self, x) return self.base + x end}) \
         return t(2)",
        42,
    );
    check_str(
        "local t = setmetatable({}, {__concat = function(a, b) return 'C' end}) return t .. 'x'",
        b"C",
    );
    check_str(
        "local t = setmetatable({}, {__concat = function(a, b) return a .. '!' end}) return 'hi' .. t",
        b"hi!",
    );
    check_int(
        "local t = setmetatable({}, {__len = function() return 99 end}) return #t",
        99,
    );
    check_str(
        "local t = setmetatable({}, {__tostring = function() return 'OBJ' end}) return tostring(t)",
        b"OBJ",
    );
    // __metatable protection
    check_str(
        "local t = setmetatable({}, {__metatable = 'locked'}) return getmetatable(t)",
        b"locked",
    );
    check_error(
        "local t = setmetatable({}, {__metatable = 'locked'}) setmetatable(t, {})",
        "protected metatable",
    );
}

#[test]
fn class_pattern_end_to_end() {
    check_int(
        "local Account = {} Account.__index = Account \
         function Account.new(b) return setmetatable({balance = b}, Account) end \
         function Account:deposit(v) self.balance = self.balance + v end \
         function Account:get() return self.balance end \
         local a = Account.new(100) a:deposit(20) a:deposit(3) return a:get()",
        123,
    );
}

#[test]
fn runtime_stack_overflow_is_caught() {
    check_bool(
        "local function f() return 1 + f() end local ok = pcall(f) return ok",
        false,
    );
    check_str(
        "local function f() return 1 + f() end local _, e = pcall(f) return e",
        b"eval:1: stack overflow",
    );
}

// ---- slice 5: goto, <close>, 5.5 global declarations, attribs ----

fn check_compile_error(src: &str, contains: &str) {
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(src) {
        Ok(v) => panic!("{src:?} unexpectedly compiled and returned {v:?}"),
        Err(e) => {
            let m = vm.error_text(&e);
            assert!(
                m.contains(contains),
                "{src:?} error {m:?} does not contain {contains:?}"
            );
        }
    }
}

#[test]
fn goto_and_labels() {
    // forward goto
    check_int("do goto done end ::done:: return 1", 1);
    check_int("local x = 1 goto skip x = 99 ::skip:: return x", 1);
    // backward goto (loop)
    check_int(
        "local n = 0 ::top:: n = n + 1 if n < 5 then goto top end return n",
        5,
    );
    // continue idiom: trailing label may skip over locals
    check_int(
        "local s = 0 for i = 1, 5 do if i % 2 == 0 then goto continue end \
         local double = i * 2 s = s + double ::continue:: end return s",
        18,
    );
    // goto out of nested blocks
    check_int("do do goto out end end ::out:: return 7", 7);
    // errors
    check_compile_error("goto nowhere", "no visible label 'nowhere'");
    check_compile_error(
        "goto later local x = 1 ::later:: return x",
        "jumps into the scope",
    );
    check_compile_error("::dup:: ::dup::", "already defined");
    // a label conflicts with a visible label in an enclosing block
    check_compile_error("::l1:: do ::l1:: end", "label 'l1' already defined");
    // a label inside a nested block is not visible to an outer goto
    check_compile_error("goto l1 do ::l1:: end", "no visible label 'l1'");
    // 5.5 scope-jump wording carries no "local"
    check_compile_error(
        "goto l1 local aa ::l1:: return aa",
        "jumps into the scope of 'aa'",
    );
    // goto leaving a block with captured locals closes them
    check_int(
        "local fs = {} local i = 1 ::top:: do local v = i fs[i] = function() return v end end \
         i = i + 1 if i <= 2 then goto top end return fs[1]() * 10 + fs[2]()",
        12,
    );
}

#[test]
fn to_be_closed() {
    // closed on normal block exit, in reverse order
    check_str(
        "local log = '' local function tracker(n) return setmetatable({}, \
           {__close = function() log = log .. n end}) end \
         do local a <close> = tracker('a') local b <close> = tracker('b') end \
         return log",
        b"ba",
    );
    // closed on error, handler sees the error object
    check_str(
        "local seen local t = setmetatable({}, {__close = function(_, e) seen = e end}) \
         local ok, err = pcall(function() local x <close> = t error('boom', 0) end) \
         return seen",
        b"boom",
    );
    // closed when a loop body iterates
    check_int(
        "local n = 0 local mt = {__close = function() n = n + 1 end} \
         for i = 1, 3 do local x <close> = setmetatable({}, mt) end return n",
        3,
    );
    // nil/false are silently accepted, others must be closable
    check_int(
        "do local x <close> = nil local y <close> = false end return 1",
        1,
    );
    check_error("local x <close> = 42", "non-closable value");
    check_compile_error(
        "local a <close>, b <close> = nil, nil",
        "multiple to-be-closed",
    );
    // close vars are read-only
    check_compile_error(
        "local x <close> = nil x = 1",
        "attempt to assign to const variable 'x'",
    );
    // normal close passes only the object (1 arg)
    check_int(
        "local n local mt = {__close = function(...) n = select('#', ...) end} \
         do local y <close> = setmetatable({}, mt) end return n",
        1,
    );
    // error close passes the object and the error object (2 args)
    check_int(
        "local n local mt = {__close = function(...) n = select('#', ...) end} \
         pcall(function() local y <close> = setmetatable({}, mt) error('e', 0) end) return n",
        2,
    );
    // an error in a __close handler chains to the next handler's error object
    check_bool(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, m) assert(m:find('@y')) error('@x') end) \
           local y <close> = c(function(_, m) assert(m == nil) error('@y') end) \
         end) \
         return msg:find('@x') ~= nil",
        true,
    );
    // a __close handler that triggers GC doesn't lose the pending error object
    check_bool(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, m) assert(m:find('@y')) error('@x') end) \
           local g <close> = c(function() collectgarbage() end) \
           local y <close> = c(function(_, m) assert(m == nil) error('@y') end) \
         end) \
         return msg:find('@x') ~= nil",
        true,
    );
    // `return f()` inside tbc scope is not a tail call (returns all results)
    check_int(
        "local function multi() return 1, 2, 3 end \
         local function bar() local _ <close> = setmetatable({}, {__close=function() end}) \
           do return multi() end end \
         local a, b, c = bar() return a + b + c",
        6,
    );
}

#[test]
fn const_attribs() {
    check_int("local x <const> = 41 return x + 1", 42);
    check_compile_error(
        "local x <const> = 1 x = 2",
        "attempt to assign to const variable 'x'",
    );
    // 5.5 collective attrib on locals
    check_compile_error(
        "local <const> a, b = 1, 2 b = 3",
        "attempt to assign to const variable 'b'",
    );
    // for-loop control variables are const in 5.5
    check_compile_error(
        "for i = 1, 3 do i = 5 end",
        "attempt to assign to const variable 'i'",
    );
    check_compile_error(
        "for k, v in pairs({}) do k = 1 end",
        "attempt to assign to const variable 'k'",
    );
    // non-control generic-for variables stay writable
    check_int(
        "for k, v in pairs({x = 1}) do v = 7 return v end return 0",
        7,
    );
    // assigning to a const captured as an upvalue in a nested function
    check_compile_error(
        "local z <const> = 1 function foo() return function() z = 2 end end",
        "attempt to assign to const variable 'z'",
    );
    // function statement assigning to a const name
    check_compile_error(
        "local foo <const> = 10 function foo() end",
        "attempt to assign to const variable 'foo'",
    );
}

#[test]
fn global_declarations_55() {
    // explicit declarations: declared names work, undeclared error
    check_int("global x = 5 return x + 1", 6);
    check_compile_error("global x = 1 return y", "variable 'y' not declared");
    check_compile_error("global x = 1 y = 2", "variable 'y' not declared");
    // collective global * restores default-style access
    check_int("global x = 1 global * y = 2 return x + y", 3);
    // global <const> *: reads fine, writes to undeclared error
    check_int(
        "global <const> * return type(print) == 'function' and 1 or 0",
        1,
    );
    check_compile_error(
        "global <const> * y = 2",
        "attempt to assign to const variable 'y'",
    );
    // explicitly declared names stay writable under a const collective
    check_int("global <const> * global n n = 41 return n + 1", 42);
    // const global declaration: initializer allowed, later writes error
    check_compile_error(
        "global z <const> = 1 z = 2",
        "attempt to assign to const variable 'z'",
    );
    // declarations are block-scoped: outside the block, default returns
    check_int("do global x x = 1 end y = 2 return y", 2);
    // global function declares its name
    check_int("global function gf() return 21 end return gf() * 2", 42);
    // locals are unaffected by strict mode
    check_int("global g local a = 3 g = a return g", 3);
    // _ENV bypass still works (declarations are purely syntactic)
    check_int("global <const> * _ENV.bypass = 9 return _ENV.bypass", 9);
}

// ---- P04 slice 1: math, table, base additions ----

#[test]
fn math_library() {
    check_int("return math.floor(2.7)", 2);
    check_int("return math.floor(-2.7)", -3);
    check_int("return math.ceil(2.1)", 3);
    check_int("return math.abs(-5)", 5);
    check_float("return math.abs(-5.5)", 5.5);
    check_int("return math.max(3, 1, 4, 1, 5)", 5);
    check_int("return math.min(3, 1, 4, 1, 5)", 1);
    check_float("return math.sqrt(16)", 4.0);
    check_float("return math.huge", f64::INFINITY);
    check_int("return math.maxinteger", i64::MAX);
    check_int("return math.mininteger", i64::MIN);
    check_str("return math.type(1)", b"integer");
    check_str("return math.type(1.0)", b"float");
    check_bool("return math.type('x') == nil", true);
    check_int("return math.tointeger(3.0)", 3);
    check_bool("return math.tointeger(3.5) == nil", true);
    check_int("return math.fmod(7, 3)", 1);
    check_int("return math.fmod(-7, 3)", -1); // fmod truncates, % floors
    check_error("return math.fmod(1, 0)", "zero");
    check_bool("return math.ult(-1, 1)", false); // -1 is huge unsigned
    check_float("return math.log(8, 2)", 3.0);
    let v = eval("local ip, fp = math.modf(3.7) return ip");
    assert!(matches!(v[0], Value::Float(f) if f == 3.0));
    // random: determinism after seeding, ranges respected
    check_bool(
        "math.randomseed(42) local a = math.random() math.randomseed(42) \
         return a == math.random()",
        true,
    );
    check_bool(
        "math.randomseed(7) for i = 1, 100 do local r = math.random(3, 9) \
         if r < 3 or r > 9 then return false end end return true",
        true,
    );
    check_error("return math.random(5, 2)", "interval is empty");
    check_error("return math.random(1, 2, 3)", "wrong number of arguments");
    // xoshiro256** conformance: PUC's exact sequence after seed 1007
    check_int(
        "math.randomseed(1007) return math.random(0)",
        0x7a7040a5a323c9d6u64 as i64,
    );
    // deg/rad/frexp/ldexp
    check_bool("return math.deg(math.pi) == 180.0", true);
    check_bool("return math.rad(180) == math.pi", true);
    check_bool(
        "local m, e = math.frexp(8.0) return m == 0.5 and e == 4",
        true,
    );
    check_bool("return math.ldexp(0.5, 4) == 8.0", true);
    // float modulo keeps fmod's sign correction for tiny denormals (no m*y
    // underflow): (-1).0 % 2.0 floors toward the divisor
    check_float("return (-1.0) % 2.0", 1.0);
    // -0.0 survives the LoadF fast path (1/-0.0 == -inf)
    check_bool("local z <const> = -0.0 return 1/z < 0", true);
    // bitwise on a non-integer field names the operand
    check_error("return math.huge << 1", "field 'huge'");
}

#[test]
fn table_library() {
    check_int("local t = {1, 2, 3} table.insert(t, 4) return t[4] + #t", 8);
    check_int(
        "local t = {1, 3} table.insert(t, 2, 2) return t[1] * 100 + t[2] * 10 + t[3]",
        123,
    );
    check_error("table.insert({}, 5, 1)", "position out of bounds");
    check_error("table.insert({}, 2, 3, 4)", "wrong number of arguments");
    // table.insert/remove use luaL_len: a non-integer __len is an error
    check_error(
        "local t = setmetatable({}, {__len = function() return 'abc' end}) table.insert(t, 1)",
        "object length is not an integer",
    );
    // table.create range/overflow checks (both args)
    check_error("table.create(0, 1 << 31)", "out of range");
    check_error("table.create(0, (1 << 31) - 1)", "table overflow");
    // table.unpack with a full-integer range must error, not hang
    check_error(
        "table.unpack({}, math.mininteger, math.maxinteger)",
        "too many results",
    );
    check_int(
        "local t = {1, 2, 3} local v = table.remove(t) return v * 10 + #t",
        32,
    );
    check_int(
        "local t = {1, 2, 3} local v = table.remove(t, 1) return v * 10 + t[1]",
        12,
    );
    check_str("return table.concat({1, 'b', 2.5}, '-')", b"1-b-2.5");
    check_str("return table.concat({}, 'x')", b"");
    check_str("return table.concat({9, 8, 7}, '', 2, 3)", b"87");
    check_error("table.concat({{}})", "invalid value");
    check_int(
        "local a, b, c = table.unpack({7, 8, 9}) return a * 100 + b * 10 + c",
        789,
    );
    check_int("return (table.unpack({1, 2, 3}, 2))", 2);
    check_int("return select('#', table.unpack({1, 2, 3}))", 3);
    check_int("local p = table.pack(4, 5, 6) return p.n * 100 + p[3]", 306);
    check_int(
        "local t = {1, 2, 3, 4, 5} table.move(t, 1, 3, 3) \
         return t[3] * 100 + t[4] * 10 + t[5]",
        123,
    );
    check_int(
        "local d = table.move({7, 8}, 1, 2, 1, {}) return d[1] * 10 + d[2]",
        78,
    );
    check_int("return #table.create(16)", 0);
    // sort: default order, comparator, strings, invalid order caught
    check_str(
        "local t = {3, 1, 4, 1, 5, 9, 2, 6} table.sort(t) return table.concat(t, '')",
        b"11234569",
    );
    check_str(
        "local t = {3, 1, 4, 1, 5} table.sort(t, function(a, b) return a > b end) \
         return table.concat(t, '')",
        b"54311",
    );
    check_str(
        "local t = {'pear', 'apple', 'fig'} table.sort(t) return table.concat(t, ',')",
        b"apple,fig,pear",
    );
    check_bool(
        "local t = {} for i = 1, 200 do t[i] = (i * 37) % 101 end table.sort(t) \
         for i = 2, 200 do if t[i - 1] > t[i] then return false end end return true",
        true,
    );
    check_error(
        "local t = {} for i = 1, 64 do t[i] = i end \
         table.sort(t, function() return true end)",
        "invalid order function",
    );
}

#[test]
fn base_additions() {
    check_int("return tonumber('42')", 42);
    check_float("return tonumber('2.5')", 2.5);
    check_int("return tonumber('0x10')", 16);
    check_bool("return tonumber('zz') == nil", true);
    check_bool("return tonumber({}) == nil", true);
    check_int("return tonumber('ff', 16)", 255);
    check_int("return tonumber('111', 2)", 7);
    check_int("return tonumber('-z', 36)", -35);
    check_bool("return tonumber('12', 2) == nil", true);
    check_error("return tonumber('1', 99)", "base out of range");
    // load
    check_int("local f = load('return 1 + 1') return f()", 2);
    check_int("local f = load('return ...', 'chunk') return f(9)", 9);
    check_bool(
        "local f, e = load('syntax ! error') return f == nil and type(e) == 'string'",
        true,
    );
    // load with custom env
    check_int(
        "local env = {x = 5} local f = load('return x', 'c', 't', env) return f()",
        5,
    );
    // collectgarbage
    check_bool("return collectgarbage('count') > 0", true);
    check_int("return collectgarbage()", 0);
    check_bool("return pairs({}) == next", true);
}

// ---- P04 slices 2+3: string library and patterns ----

#[test]
fn string_core() {
    check_int("return string.len('hello')", 5);
    check_int("return ('hello'):len()", 5); // method syntax via string metatable
    check_str("return ('hello'):sub(2, 4)", b"ell");
    check_str("return ('hello'):sub(-3)", b"llo");
    check_str("return ('hello'):sub(2)", b"ello");
    check_str("return ('hello'):sub(4, 2)", b"");
    check_str("return ('hello'):sub(-100, 100)", b"hello");
    check_str("return ('aBc'):upper()", b"ABC");
    check_str("return ('aBc'):lower()", b"abc");
    check_str("return ('ab'):rep(3)", b"ababab");
    check_str("return ('ab'):rep(3, '-')", b"ab-ab-ab");
    check_str("return ('ab'):rep(0)", b"");
    check_str("return ('abc'):reverse()", b"cba");
    check_int("return ('A'):byte()", 65);
    check_int("return select('#', ('abc'):byte(1, 3))", 3);
    check_str("return string.char(104, 105)", b"hi");
    check_error("return string.char(300)", "value out of range");
    // numbers coerce in string functions
    check_int("return string.len(123)", 3);
}

#[test]
fn string_find_and_match() {
    check_int("return (string.find('hello', 'll'))", 3);
    check_int("return select(2, string.find('hello', 'll'))", 4);
    check_bool("return string.find('hello', 'xyz') == nil", true);
    check_int("return (string.find('hello', 'l+'))", 3);
    check_int("return (string.find('a.b', '.', 1, true))", 2); // plain
    check_int("return (string.find('hello', 'l', -2))", 4); // negative init
    check_str("return (string.match('key=val', '(%w+)=(%w+)'))", b"key");
    check_str(
        "return select(2, string.match('key=val', '(%w+)=(%w+)'))",
        b"val",
    );
    check_str("return string.match('hello 42!', '%d+')", b"42");
    check_bool("return string.match('abc', '%d') == nil", true);
    check_int("return string.match('abc', '()b')", 2); // position capture
    check_str("return string.match('  trim  ', '^%s*(.-)%s*$')", b"trim");
    check_error("return string.match('x', '%')", "malformed pattern");
}

#[test]
fn string_gmatch() {
    check_int(
        "local n = 0 for w in ('one two three'):gmatch('%a+') do n = n + 1 end return n",
        3,
    );
    check_str(
        "local t = {} for k, v in ('a=1,b=2'):gmatch('(%w+)=(%w+)') do t[#t+1] = k .. v end \
         return table.concat(t, ' ')",
        b"a1 b2",
    );
    // standalone iterator calls work (closure state, no generic for)
    check_str(
        "local it = ('x y'):gmatch('%a') local a = it() local b = it() return a .. b",
        b"xy",
    );
    // empty matches make progress
    check_int(
        "local n = 0 for _ in ('abc'):gmatch('x*') do n = n + 1 end return n",
        4,
    );
}

#[test]
fn string_gsub() {
    check_str("return (('hello world'):gsub('o', '0'))", b"hell0 w0rld");
    check_int("return select(2, ('hello'):gsub('l', 'L'))", 2);
    check_str("return (('hello'):gsub('l', 'L', 1))", b"heLlo");
    check_str("return (('abc'):gsub('(%a)', '%1%1'))", b"aabbcc");
    check_str(
        "return (('key=val'):gsub('(%w+)=(%w+)', '%2=%1'))",
        b"val=key",
    );
    check_str("return (('ab'):gsub('b', '100%%'))", b"a100%");
    // table replacement
    check_str(
        "return (('$name is $age'):gsub('%$(%w+)', {name = 'lua', age = 30}))",
        b"lua is 30",
    );
    // function replacement; false/nil keeps the original
    check_str(
        "return (('1 2 3'):gsub('%d', function(d) return tonumber(d) * 2 end))",
        b"2 4 6",
    );
    check_str(
        "return (('keep drop'):gsub('%a+', function(w) if w == 'drop' then return 'X' end end))",
        b"keep X",
    );
    // empty pattern progress
    check_str("return (('ab'):gsub('', '-'))", b"-a-b-");
    check_error("return ('x'):gsub('x', '%9')", "invalid capture index");
    check_str("return (('x'):gsub('x', {}))", b"x"); // table lookup nil keeps original
    // 5.3.3 empty-match semantics: no double replacement after a non-empty one
    check_str("return (('a b cd'):gsub(' *', '-'))", b"-a-b-c-d-");
    check_int("return select(2, ('a b cd'):gsub(' *', '-'))", 5);
    // table replacement honours __index
    check_str(
        "local t = setmetatable({}, {__index = function(_, k) return k:upper() end}) \
         return (('a bb'):gsub('%a+', t))",
        b"A BB",
    );
    // no-change reuse: the count still reflects matches even when nothing changed
    check_int("return select(2, ('aaa'):gsub('.', {}))", 3);
}

#[test]
fn string_gmatch_init_and_empty() {
    // 1-based init parameter (5.4)
    check_int(
        "local s = 0 for k in ('10 20 30'):gmatch('%d+', 3) do s = s + tonumber(k) end return s",
        50,
    );
    // negative init counts from the end
    check_int(
        "local s = 0 for k in ('11 21 31'):gmatch('%d+', -2) do s = s + tonumber(k) end return s",
        31,
    );
    // position-capture empty matches advance cleanly (PUC lastmatch rule)
    check_str(
        "local r = '' local i = 1 local sub = 'a b' \
         for p, e in sub:gmatch('()%s*()') do r = r .. sub:sub(i, p - 1) .. '-' i = e end \
         return r",
        b"-a-b-",
    );
}

#[test]
fn pattern_classes_balanced_frontier() {
    check_str("return string.match('foo (bar) baz', '%b()')", b"(bar)");
    check_str("return string.match('THE quick', '%f[%l]%a+')", b"quick");
    check_str("return string.match('abc123', '%a+')", b"abc");
    check_str("return string.match('abc123', '%A+')", b"123"); // complement... wait %A = non-alpha → 123
    check_str("return string.match('a-b', '%p')", b"-");
    check_str("return string.match('x\\ty', '%s')", b"\t");
    check_str("return string.match('abcabc', '(a%w+)%1')", b"abc"); // backref... wait (a%w+) greedy
    check_str("return string.match('[x]', '%[(%a)%]')", b"x");
}

// ---- P04 slice 4: string.format ----

#[test]
fn string_format() {
    check_str("return string.format('%d', 42)", b"42");
    check_str("return string.format('%d', -42)", b"-42");
    check_str("return string.format('%5d', 42)", b"   42");
    check_str("return string.format('%-5d|', 42)", b"42   |");
    check_str("return string.format('%05d', 42)", b"00042");
    check_str("return string.format('%+d %+d', 5, -5)", b"+5 -5");
    check_str("return string.format('%x', 255)", b"ff");
    check_str("return string.format('%X', 255)", b"FF");
    check_str("return string.format('%#x', 255)", b"0xff");
    check_str("return string.format('%o', 8)", b"10");
    check_str("return string.format('%x', -1)", b"ffffffffffffffff");
    check_str("return string.format('%c%c', 104, 105)", b"hi");
    check_str("return string.format('%s=%s', 'a', 1)", b"a=1");
    check_str("return string.format('%10s|', 'hi')", b"        hi|");
    check_str("return string.format('%-10s|', 'hi')", b"hi        |");
    check_str("return string.format('%.3s', 'hello')", b"hel");
    check_str("return string.format('%f', 1.5)", b"1.500000");
    check_str("return string.format('%.2f', 3.14159)", b"3.14");
    check_str("return string.format('%.0f', 2.5)", b"2");
    check_str("return string.format('%e', 1500.0)", b"1.500000e+03");
    check_str("return string.format('%.2E', 0.0001)", b"1.00E-04");
    check_str("return string.format('%g', 100000.0)", b"100000");
    check_str("return string.format('%g', 1e+20)", b"1e+20");
    check_str("return string.format('%g', 0.0001)", b"0.0001");
    check_str("return string.format('%g', 0.00001)", b"1e-05");
    check_str("return string.format('%.3g', 3.14159)", b"3.14");
    check_str("return string.format('%g', 2.0)", b"2");
    check_str("return string.format('%a', 1.0)", b"0x1p+0");
    check_str("return string.format('%a', 0.5)", b"0x1p-1");
    check_str("return string.format('%a', 3.0)", b"0x1.8p+1");
    check_str("return string.format('%d%%', 99)", b"99%");
    // %q round-trips. PUC addquoted escapes a newline as backslash + a real
    // newline (not `\n`), so it reads back as the same string.
    check_str(
        "return string.format('%q', 'a\\nb\"c\\\\d')",
        b"\"a\\\nb\\\"c\\\\d\"",
    );
    // %q of math.mininteger uses a hex literal (decimal would reparse as float)
    check_str(
        "return string.format('%q', math.mininteger)",
        b"0x8000000000000000",
    );
    check_str("return string.format('%q', 7)", b"7");
    check_bool(
        "return load('return ' .. string.format('%q', 'x\\0y'))() == 'x\\0y'",
        true,
    );
    check_str("return string.format('%q', 0/0)", b"(0/0)");
    check_str("return string.format('%q', 2.0)", b"2.0");
    // tostring path honors __tostring in %s
    check_str(
        "local t = setmetatable({}, {__tostring = function() return 'T' end}) \
         return string.format('[%s]', t)",
        b"[T]",
    );
    // errors
    check_error(
        "return string.format('%d', 1.5)",
        "no integer representation",
    );
    check_error("return string.format('%d')", "no value");
    check_error("return string.format('%k', 1)", "invalid conversion");
}

#[test]
fn utf8_library() {
    check_str("return utf8.char(72, 105)", b"Hi");
    check_str("return utf8.char(0x4F60, 0x597D)", "你好".as_bytes());
    check_int("return utf8.len('héllo')", 5);
    check_int("return utf8.len('你好')", 2);
    check_int("return (utf8.codepoint('你好'))", 0x4F60);
    check_int("return select(2, utf8.codepoint('你好', 1, -1))", 0x597D);
    check_int("return (utf8.offset('你好', 2))", 4);
    // 5.5: offset also returns the character's final byte position
    check_int("return select(2, utf8.offset('你好', 1))", 3);
    check_int("return (utf8.offset('你好x', -1))", 7);
    check_int(
        "local n = 0 for p, c in utf8.codes('a你b') do n = n + 1 end return n",
        3,
    );
    check_int(
        "local last for p in utf8.codes('a你b') do last = p end return last",
        5,
    );
    // invalid sequences
    check_bool("return utf8.len('\\xFF') == nil", true);
    check_int("return select(2, utf8.len('a\\xFFb'))", 2);
    check_error("return utf8.codepoint('\\x80')", "invalid UTF-8 code");
    check_bool("return utf8.charpattern ~= nil", true);
    // offset(s, 0, i): start and end byte positions of the char containing i
    check_int("return (utf8.offset('a你b', 0, 3))", 2);
    check_int("return select(2, utf8.offset('a你b', 0, 3))", 4);
    // bounds errors (position out of bounds / continuation byte)
    check_error("return utf8.offset('abc', 1, 5)", "position out of bounds");
    check_error("return utf8.offset('', 1, 2)", "position out of bounds");
    check_error("return utf8.offset('\\x80', 1)", "continuation byte");
    check_error("return utf8.len('abc', 0, 2)", "out of bounds");
    check_error("return utf8.len('abc', 1, 4)", "out of bounds");
}

#[test]
fn debug_getinfo_name() {
    // name/namewhat recovered from the caller's call instruction (getobjname);
    // local-variable debug names make a local function nameable
    // non-tail calls so a caller frame exists to inspect (a tail call drops
    // the name, like PUC)
    check_str(
        "local function F() return debug.getinfo(1, 'n').name end local r = F() return r",
        b"F",
    );
    check_str(
        "local t = {} function t.m() return debug.getinfo(1).name end local r = t.m() return r",
        b"m",
    );
    check_str(
        "function glob() return debug.getinfo(1).name end local r = glob() return r",
        b"glob",
    );
    // a directly-invoked anonymous function has no recoverable name
    check_bool(
        "local r = (function() return debug.getinfo(1, 'n').name end)() return r == nil",
        true,
    );
}

#[test]
fn debug_getinfo_c_frame_boundary() {
    // debug.getinfo level traversal sees a synthetic C frame at a call_value
    // boundary: from inside a __close handler (invoked by the close machinery),
    // level 1 is the handler (Lua) and level 2 is "C".
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local what \
         local function foo() \
           local x <close> = c(function() what = debug.getinfo(2).what end) \
           error('e') \
         end \
         pcall(foo) \
         return what",
        b"C",
    );
    // a function called through pcall sees pcall as a C frame at level 2
    check_str(
        "local seen \
         local function f() seen = debug.getinfo(2).what end \
         pcall(f) \
         return seen",
        b"C",
    );
}

#[test]
fn debug_getinfo_source_lines_options() {
    // linedefined / lastlinedefined span the function from `function` to `end`;
    // the function-value form has an empty namewhat and no name.
    check_str(
        "local function test (a) \n\
           local x = a \n\
           return x \n\
         end \n\
         local i = debug.getinfo(test, 'S') \n\
         return i.what .. ',' .. i.linedefined .. ',' .. i.lastlinedefined \n\
            .. ',' .. i.namewhat .. ',' .. tostring(i.name)",
        b"Lua,1,4,,nil",
    );
    // "L" yields activelines (a set keyed by line); body and closing-`end` lines
    // are active, the `function` header line is not. A C function has none.
    check_bool(
        "local function test (a) \n\
           local x = a \n\
           return x \n\
         end \n\
         local act = debug.getinfo(test, 'L').activelines \n\
         return act[2] and act[4] and not act[1] and not act[5] \n\
            and debug.getinfo(print, 'L').activelines == nil",
        true,
    );
    // an out-of-range stack level is nil; a bad option char (or leading '>')
    // raises.
    check_bool(
        "return debug.getinfo(1000) == nil \
            and not pcall(debug.getinfo, print, 'X') \
            and not pcall(debug.getinfo, 1, '>')",
        true,
    );
    // short_src renders a long string source with luaO_chunkid truncation
    // ([string "..."]) and a one-liner verbatim.
    check_bool(
        "local f = load('return 1') \
         local g = load('return ' .. ('p'):rep(400)) \
         return debug.getinfo(f).short_src == '[string \"return 1\"]' \
            and string.find(debug.getinfo(g).short_src, '^%[string [^\\n]*%.%.%.\"%]$') ~= nil",
        true,
    );
    // a stripped binary chunk carries no line info: empty activelines.
    check_int(
        "local f = load(string.dump(load('print(1)'), true)) \
         local act = debug.getinfo(f, 'L').activelines \
         return #act",
        0,
    );
}

#[test]
fn debug_line_hook() {
    // debug.sethook(f, "l") fires a "line" event per source line; the hook runs
    // with events disabled and is cleared by sethook() with no args. PUC 5.5
    // `traceexec` uses `npci <= L->oldpc`, and `lua_sethook` does NOT touch
    // oldpc, so the very first step after the install fires (db.lua :322
    // depends on this: install + four statement lines == count == 4). luna
    // mirrors that by arming `hook_oldpc` to a sentinel in `install_hook`, so
    // the install-line's first follow-up step fires — here that is line 4
    // (`local a = 1`).
    check_str(
        "local out = {} \n\
         local function h(ev, ln) out[#out + 1] = ev .. ':' .. ln end \n\
         debug.sethook(h, 'l') \n\
         local a = 1 \n\
         local b = 2 \n\
         debug.sethook() \n\
         return table.concat(out, ',')",
        b"line:4,line:5,line:6",
    );
    // gethook reports the installed hook, then nil once cleared
    check_bool(
        "local function h() end \
         debug.sethook(h, 'l') \
         local got = debug.gethook() \
         debug.sethook() \
         return got == h and debug.gethook() == nil",
        true,
    );
}

#[test]
fn debug_line_hook_does_not_recurse_into_itself() {
    // CB IO follow-up regression: an `||` / `&&` precedence bug in the
    // dispatcher's count+line predicate (exec.rs ~6625) left the
    // `!self.in_hook` guard only gating the rust-hook arm. With a Lua hook
    // installed and the hook body executing any Lua bytecode (e.g. an
    // `assert(...)` call), the hook would re-fire inside itself → unbounded
    // recursion → stack overflow on PUC db.lua line 14 / 16 / 22 (the
    // `assert(event == 'line')` inside the line-hook body).
    //
    // Repro mirrors PUC db.lua `test`: install a Lua line hook whose body
    // dispatches Lua bytecode (table writes + assert) — if the guard works,
    // we get a finite list of line events; if it doesn't, the hook recurses
    // through `assert` and overflows the stack.
    check_int(
        "local n = 0 \n\
         local function h(ev) \n\
           assert(ev == 'line') \n\
           n = n + 1 \n\
         end \n\
         debug.sethook(h, 'l') \n\
         local a = 1 \n\
         local b = 2 \n\
         local c = 3 \n\
         debug.sethook() \n\
         return n",
        // Without the fix: stack overflow before sethook() runs.
        // With the fix: a finite number of line events (one per source line
        // executed under the hook). We do not pin the exact count — the
        // PUC `traceexec` discipline already has its own coverage in
        // `debug_line_hook` / `debug_line_table_precision` — we just need
        // n > 0 and the program to terminate.
        // check_int requires an exact value; this VM emits 4 line events
        // under PUC `npci <= oldpc` semantics (install-step + 3 statement
        // lines; sethook clear is on the same line as the call site so
        // changedline is false there). The point of the test is that the
        // program *terminates with a finite count* — pre-fix it would have
        // stack-overflowed before reaching `return n`.
        4,
    );
}

#[test]
fn debug_line_table_precision() {
    // line traces match PUC's per-instruction line table across constructs, the
    // way db.lua tests them (hook installed + chunk run on one line). The chunk
    // is a string literal passed to load().
    // The wrapper is one line, so the install-statement and the load() call
    // share a source line: `changedline` is false at the wrapper boundary,
    // so it does not fire — the trace only contains the loaded chunk's per-
    // instruction line-table expectations (PUC db.lua semantics).
    let trace = |chunk: &str| -> String {
        format!(
            "local l = {{}} \
             debug.sethook(function(e, n) l[#l + 1] = n end, 'l'); \
             load({chunk})(); debug.sethook() \
             return table.concat(l, ',')"
        )
    };
    // if/else: condition line, taken branch, chunk's final `end` line.
    check_str(
        &trace("'if\\nmath.sin(1)\\nthen\\n a=1\\nelse\\n a=2\\nend\\n'"),
        b"2,4,7",
    );
    // a numeric for re-fires the `for` line on each iteration (FORLOOP back-edge)
    check_str(&trace("'for i=1,3 do\\n a=i\\nend\\n'"), b"1,2,1,2,1,2,1,3");
    // a local function's closure-creation lands on its `end` line (PUC luaK_code
    // uses the just-consumed token's line)
    check_str(
        &trace("'local function foo()\\nend\\nfoo()\\nA=1\\nA=2\\nA=3\\n'"),
        b"2,3,2,4,5,6",
    );
}

#[test]
fn debug_upvalue_order_and_id() {
    // upvalue indices follow PUC's restassign ordering: a name first seen on an
    // assignment's left captures its index before one first seen on the right.
    // `a = 10 + b` (a on the left) → a is upvalue 1, b is upvalue 2.
    check_str(
        "local a, b = 1, 2 \
         local f = function (y) if y then a = 10 + b else return a end end \
         local n1 = (debug.getupvalue(f, 1)) \
         local n2 = (debug.getupvalue(f, 2)) \
         return n1 .. ',' .. n2",
        b"a,b",
    );
    // setupvalue targets the right slot (upvalue 1 = 'a') and returns its name
    check_int(
        "local a, b = 0, 5 \
         local f = function () return a + b end \
         local nm = debug.setupvalue(f, 1, 7) \
         return (nm == 'a') and f() or -1",
        12,
    );
    // upvalueid: out-of-range yields nil (not an error, unlike upvaluejoin);
    // distinct upvalues have distinct ids, shared ones compare equal; it also
    // works on a C closure (the gmatch iterator).
    check_bool(
        "local a, b = 1, 2 \
         local f = function () return a + b end \
         local g = function () return b + a end \
         return debug.upvalueid(f, 3) == nil \
            and debug.upvalueid(f, 1) ~= debug.upvalueid(f, 2) \
            and debug.upvalueid(f, 1) == debug.upvalueid(g, 2) \
            and debug.upvalueid(string.gmatch('x', 'x'), 1) ~= nil \
            and (not pcall(debug.upvaluejoin, f, 9, g, 1))",
        true,
    );
}

#[test]
fn type_error_varinfo() {
    // index / call / arith type errors name the offending operand (getobjname)
    check_error(
        "local x return x.y",
        "attempt to index a nil value (local 'x')",
    );
    check_error("return undefined_glob.y", "(global 'undefined_glob')");
    check_error("local t = {} return t.a.b", "(field 'a')");
    check_error(
        "local f local r = f()",
        "attempt to call a nil value (local 'f')",
    );
    check_error(
        "local r = nope()",
        "attempt to call a nil value (global 'nope')",
    );
    check_error(
        "local n return n + 1",
        "attempt to perform arithmetic on a nil value (local 'n')",
    );
    // an upvalue operand is named too
    check_error(
        "local up local function g() return up.x end return g()",
        "(upvalue 'up')",
    );
}

#[test]
fn error_object_edges() {
    // error() / error(nil): a nil error object becomes "<no error object>"
    check_str(
        "local ok, msg = pcall(function() error() end) return msg",
        b"<no error object>",
    );
    check_str(
        "local ok, msg = pcall(function() error(nil) end) return msg",
        b"<no error object>",
    );
    // a non-nil error object is preserved (string with position prefix)
    check_bool(
        "local ok, msg = pcall(function() error('boom', 0) end) return msg == 'boom'",
        true,
    );
    // tostring/tonumber require an argument (luaL_checkany)
    check_error(
        "return tostring()",
        "bad argument #1 to 'tostring' (value expected)",
    );
    check_error(
        "return tonumber()",
        "bad argument #1 to 'tonumber' (value expected)",
    );
    check_str("return tostring(1)", b"1");
}

#[test]
fn metatables_basic_types_and_len_arity() {
    // unary metamethods receive the operand twice (PUC); __len here returns
    // the second arg to observe arity
    check_int(
        "local t = setmetatable({}, {__len = function(a, b) return (a == b) and 7 or 0 end}) \
         return #t",
        7,
    );
    // debug.setmetatable sets the shared metatable for a basic type
    check_int(
        "debug.setmetatable(10, {__index = function(a, b) return a + b end}) \
         local r = (10)[3] debug.setmetatable(10, nil) return r",
        13,
    );
    check_bool(
        "debug.setmetatable(true, {__index = {hi = 42}}) \
         local r = (true).hi debug.setmetatable(true, nil) return r == 42",
        true,
    );
    // getmetatable reflects the per-type metatable
    check_bool(
        "local mt = {} debug.setmetatable(1.5, mt) \
         local ok = getmetatable(-2) == mt debug.setmetatable(1.5, nil) return ok",
        true,
    );
}

#[test]
fn table_move_and_sort_guards() {
    // table.move honours __index (read) and __newindex (write)
    check_str(
        "local src = setmetatable({}, {__index = function(_, k) return ('%d'):format(k) end}) \
         local dst = table.move(src, 1, 3, 1, {}) return dst[1] .. dst[2] .. dst[3]",
        b"123",
    );
    // range/overflow guards instead of looping
    check_error(
        "table.move({}, 0, math.maxinteger, 1)",
        "too many elements to move",
    );
    check_error(
        "table.move({}, 1, 2, math.maxinteger)",
        "destination wrap around",
    );
    // table.sort honours __len and rejects a too-big array
    check_error(
        "table.sort(setmetatable({}, {__len = function() return math.maxinteger end}))",
        "array too big",
    );
    // an invalid order function is detected even for small arrays
    check_error(
        "table.sort({1,2,3,4}, function() return true end)",
        "invalid order function",
    );
    // a valid sort still works
    check_int(
        "local t = {3,1,2,5,4} table.sort(t) return t[1]*10000+t[2]*1000+t[3]*100+t[4]*10+t[5]",
        12345,
    );
}

#[test]
fn global_declarations_55_edges() {
    // `global` is a contextual keyword: an ordinary identifier unless it leads
    // a declaration (followed by a name / '*' / function / attribute).
    check_int("global = 1; return global", 1);
    check_int("local global = 41; return global + 1", 42);

    // explicit declaration + strict default once any global is declared
    check_compile_error("global none; X = 1", "variable 'X'");
    check_compile_error(
        "global none; local function f() XXX = 1 end",
        "variable 'XXX'",
    );

    // a `global *` collective re-opens implicit globals
    check_int("global *; Y = 7; return _ENV.Y", 7);

    // const globals are read-only after declaration, writable as the defining
    // initializer
    check_compile_error(
        "global<const> foo; function foo() end",
        "assign to const variable 'foo'",
    );
    check_int("global<const> a = 5; return _ENV.a", 5);

    // close attribute is rejected on globals with the 5.5 wording
    check_compile_error("global X<close>", "cannot be to-be-closed");
    check_compile_error("global <close> *", "cannot be to-be-closed");

    // `_ENV` pulled into a global declaration makes every global access error
    check_compile_error("global _ENV, a; a = 10", "variable 'a'");

    // an inner `global X` shadows an enclosing local X for that scope only
    check_int(
        "local X = 10; do global X; X = 20 end; return X * 1000 + _ENV.X",
        10020,
    );

    // an initializer reads the enclosing scope, not the global being defined
    check_int(
        "local a, b = 100, 200; do global a, b = a, b end; return _ENV.a + _ENV.b",
        300,
    );

    // a defining write to an already-existing global is a runtime error
    check_error(
        "_ENV.dup = 1; global dup = 2",
        "global 'dup' already defined",
    );
    check_error(
        "_ENV.fdup = 1; global function fdup() end",
        "global 'fdup' already defined",
    );
}

#[test]
fn goto_scope_over_declarations() {
    // a goto cannot jump over a local declaration into its scope
    check_compile_error("goto l1; local aa ::l1:: ::l2:: return 0", "scope of 'aa'");
    // ...nor over a `global *` collective marker ('*' in the wording)
    check_compile_error("goto l2; global *; ::l1:: ::l2:: return 0", "scope of '*'");
    // repeat-until keeps body locals alive through the condition, so a goto to
    // a trailing label lands in their scope
    check_compile_error(
        "repeat if x then goto cont end local xuxu = 10 ::cont:: until xuxu < 1",
        "scope of 'xuxu'",
    );
}

#[test]
fn lexer_string_escape_near_tokens() {
    // PUC near-token = raw source of the string read so far; decimal-too-large
    // includes the char after the digits, utf8-too-large stops at the digit.
    check_compile_error(r#"return "\999""#, r#"near '\999"'"#);
    check_compile_error(r#"return "abc\u{100000000}""#, r#"near 'abc\u{100000000'"#);
    check_compile_error(r#"return "abc\u{11r""#, r#"near 'abc\u{11r'"#);
    check_compile_error(r#"return "abc\u""#, r#"near 'abc\u"'"#);
    // unfinished string reports the <eof> token
    check_compile_error("return 'alo", "unfinished string near <eof>");
}

#[test]
fn string_constants_are_interned_per_chunk() {
    // identical literals — even long (>40 bytes) ones, across nested functions —
    // share one object, so their %p addresses compare equal.
    let long = "0123456789012345678901234567890123456789012345"; // 46 bytes
    check_int(
        &format!(
            "local a <const> = {long:?} \
             local function f() return {long:?} end \
             return (string.format('%p', a) == string.format('%p', f())) and 1 or 0"
        ),
        1,
    );
    // a `#` chunk is the length operator, not a shebang (string load never strips)
    check_compile_error("#x = 1", "unexpected symbol");
}

#[test]
fn comparison_and_for_error_wording() {
    // PUC luaG_ordererror: matching types report "two X values"
    check_error(
        "return print < print",
        "attempt to compare two function values",
    );
    check_error("return {} < {}", "attempt to compare two table values");
    check_error("return 1 < 'x'", "attempt to compare number with string");
    // PUC luaG_forerror: "bad 'for' <what> (number expected, got <type>)"
    check_error(
        "for i = 1, 'x', 10 do end",
        "bad 'for' limit (number expected, got string)",
    );
    check_error(
        "for i = 1, 10, print do end",
        "bad 'for' step (number expected, got function)",
    );
}

#[test]
fn parser_near_token_and_goto_lines() {
    // <eof> is the only unquoted near-token (PUC luaX_token2str)
    check_compile_error("local a = {", "near <eof>");
    check_compile_error("local a = (1", "near <eof>");
    // a normal token is single-quoted
    check_compile_error("local a = 1 +", "near <eof>");
    // goto/label diagnostics carry the relevant source line (PUC)
    check_compile_error("::A:: a = 1 ::A::", "already defined on line 1");
    check_compile_error(
        "goto A do ::A:: end",
        "no visible label 'A' for <goto> at line 1",
    );
}

#[test]
fn string_dump_round_trips() {
    // basic round-trip: dump a function, reload, call
    check_int(
        "local s = string.dump(load('return 6*7')) return load(s)()",
        42,
    );
    // strip flag still produces a loadable chunk
    check_int("return load(string.dump(load('return 40+2'), true))()", 42);
    // constants of every serialisable kind survive
    check_str(
        "local f = load([[return ('a'..1)..(2.5)..tostring(true)..tostring(nil)]]) \
         return load(string.dump(f))()",
        b"a12.5truenil",
    );
    // a reloaded chunk still reaches globals through its fresh _ENV upvalue
    check_int(
        "gv = 9 local s = string.dump(load('return gv')) return load(s)()",
        9,
    );
    // nested function prototypes round-trip
    check_int(
        "local src = 'local function k() return 5 end return k()+k()' \
         return load(string.dump(load(src)))()",
        10,
    );
    // only Lua functions can be dumped
    check_error("string.dump(print)", "unable to dump given function");
}

#[test]
fn dbg_frame_inserts_tail_synthetic_under_51() {
    // PUC 5.1's `lua_getstack` reports a synthetic CIST_TAIL level between
    // each tail-called Lua frame and its caller: `getinfo(2).what == "tail"`
    // from inside a tail-called function, with the real caller at level 3.
    // 5.2+ retired the synthetic shape — `istailcall` becomes a flag on
    // the real frame and `getinfo(2).func == g1`. 5.1 db.lua :334-:343 vs
    // 5.5 db.lua :625-:628 pin each shape.
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.eval(
        "local function f (x) \
             if x then \
                 assert(debug.getinfo(2).what == 'tail') \
                 assert(not pcall(getfenv, 3)) \
                 assert(debug.getinfo(3, 'f').func == g1) \
             end \
         end \
         function g(x) return f(x) end \
         function g1(x) g(x) end \
         local function h(x) local f = g1; return f(x) end \
         h(true)",
    )
    .expect("5.1 tail-call shape");

    let mut vm5 = Vm::new(LuaVersion::Lua55);
    vm5.eval(
        "local function f (x) \
             if x then \
                 assert(debug.getinfo(1, 't').istailcall == true) \
                 local tail = debug.getinfo(2) \
                 assert(tail.func == g1 and tail.istailcall == true) \
             end \
         end \
         function g(x) return f(x) end \
         function g1(x) g(x) end \
         local function h(x) local f = g1; return f(x) end \
         h(true)",
    )
    .expect("5.5 tail-call shape");
}

#[test]
fn tail_call_to_native_keeps_caller_frame() {
    // PUC's `OP_TAILCALL` only collapses Lua→Lua activations. A tail call
    // to a C function (`return getfenv()`, `return os.time()`, etc.) runs
    // the C function under the *current* Lua frame so a level-1 debug
    // lookup still resolves to the caller. luna previously popped the
    // frame unconditionally, leaving native targets to fall back to the
    // thread's globals; 5.1 closure.lua :177 pinned this with
    // `return getfenv()` inside a coroutine whose `setfenv(0, env)` only
    // retunes the thread.
    let mut vm = Vm::new(LuaVersion::Lua51);
    let r = vm
        .eval(
            "local function foo (a) \
                 setfenv(0, a) \
                 coroutine.yield(getfenv()) \
                 return getfenv() \
             end \
             local f = coroutine.wrap(foo) \
             local a = {} \
             local r1 = f(a) \
             local _, r2 = pcall(f) \
             return r1 == _G, r2 == _G",
        )
        .expect("eval");
    assert!(
        matches!(r.first(), Some(Value::Bool(true))),
        "r1 == _G slot: {:?}",
        r.first()
    );
    assert!(
        matches!(r.get(1), Some(Value::Bool(true))),
        "r2 == _G slot (tail call to native must preserve caller frame): {:?}",
        r.get(1)
    );
}

#[test]
fn weak_table_dead_key_does_not_alias_reused_alloc() {
    // PUC `setdeadkey` analogue: when GC sweeps a collectable key out of a
    // weak table the Gc pointer in the node is left dangling, and the
    // freed memory is fair game for the next allocator request. A naïve
    // `find_node` then risks `raw_eq` matching the dangling pointer
    // against a freshly-allocated object whose Gc landed at the same
    // address — gc.lua 5.5 :459-:478 hit that ~12% of the time (the
    // swept B-string's slot chained into A's slot, and the post-sweep
    // `a[k] = nil` clobbered the dead slot's val instead of A's). Hammer
    // the same shape: insert a soon-to-be-swept long string + a live
    // string + a soon-to-be-swept table key, gc, then `a[k] = nil` and
    // verify the live entry actually clears.
    check_int(
        "local rounds = 32 \
         for _ = 1, rounds do \
             local a = setmetatable({}, {__mode = 'kv'}) \
             a[string.rep('a', 2^14)] = 1 \
             a[string.rep('b', 2^14)] = {} \
             a[{}] = 2 \
             collectgarbage() \
             local k = next(a) \
             a[k] = nil \
             collectgarbage() \
             assert(next(a) == nil) \
         end \
         return rounds",
        32,
    );
}

#[test]
fn weak_kv_table_marks_surviving_string_keys() {
    // Lua manual §2.5.4: strings in weak tables are not collected as long
    // as their entry is. PUC `iscleared` implements that by marking the
    // string during the scan; the FAILURE mode is a `__mode='kv'` table
    // with a string key and an alive value whose key string gets swept
    // because nothing else holds it (gc.lua 5.5 :459-:478 was an
    // intermittent ~20% failure before this fix). Hammer the scenario
    // with full GCs so the bug, if regressed, surfaces deterministically.
    check_int(
        "local rounds = 16 \
         for _ = 1, rounds do \
             local a = setmetatable({}, {__mode = 'kv'}) \
             a[string.rep('a', 1024)] = 25 \
             a[string.rep('b', 1024)] = {} \
             a[{}] = 14 \
             collectgarbage() \
             local k, v = next(a) \
             assert(type(k) == 'string' and k:sub(1,1) == 'a' and v == 25, \
                    'expected (\"a*\", 25), got ('..tostring(k)..', '..tostring(v)..')') \
             assert(next(a, k) == nil) \
         end \
         return rounds",
        16,
    );
}

#[test]
fn coroutine_resume_refuses_too_many_results() {
    // PUC `auxresume` (lcorolib.c) calls `lua_checkstack(L, nres + 1)` on
    // the parent thread before transferring the coroutine's return values.
    // A coroutine that produces near-`LUAI_MAXSTACK` values into its own
    // stack still cannot deliver them when the caller's stack has no room.
    // 5.3 coroutine.lua :530's `for j in {lim-10, lim-5, …}` series pins
    // this — every j from `lim - 10` upward must fail.
    check_str(
        "local lim = 1000000 \
         local out = {} \
         for _, j in ipairs{lim - 10, lim - 5, lim - 1, lim, lim + 1} do \
             local co = coroutine.create(function () \
                 local t = {} \
                 for i = 1, j do t[i] = i end \
                 return table.unpack(t) \
             end) \
             local r = coroutine.resume(co) \
             out[#out + 1] = tostring(r) \
         end \
         return table.concat(out, ',')",
        b"false,false,false,false,false",
    );
}

#[test]
fn le_synthesis_via_lt_is_yieldable_53() {
    // ≤5.3 `a <= b` falls back to `not __lt(b, a)` when neither operand
    // carries `__le`. The metamethod call has to stay yieldable so a
    // coroutine running inside a `<=` operator can suspend in `__lt` and
    // resume cleanly — coroutine.lua 5.3 :599 pins this.
    let mut vm = Vm::new(LuaVersion::Lua53);
    let r = vm
        .eval(
            "local mt = { __lt = function (a, b) \
                 coroutine.yield(nil, 'lt'); return a.x < b.x end } \
             local a = setmetatable({x=10}, mt) \
             local b = setmetatable({x=12}, mt) \
             local co = coroutine.wrap(function () return a <= b end) \
             local _, stat = co() \
             local r = co() \
             return stat, r",
        )
        .expect("eval");
    assert!(
        matches!(r.first(), Some(Value::Str(s)) if s.as_bytes() == b"lt"),
        "stat slot: {:?}",
        r.first()
    );
    assert!(
        matches!(r.get(1), Some(Value::Bool(true))),
        "result slot: {:?}",
        r.get(1)
    );
}

#[test]
fn load_seeds_globals_only_when_one_upvalue() {
    // PUC `lua_load` writes the globals table into the loaded closure's
    // first upvalue cell *only* when the closure has exactly one upvalue
    // (the main-chunk `_ENV` case). A dumped non-main function with
    // multiple upvalues keeps every cell at nil — 5.2 calls.lua :293's
    // `assert(x() == nil)` reads the dumped `a` upvalue and must see nil
    // rather than the globals table leaking in.
    check_bool(
        "local a, b = 20, 30 \
         local d = string.dump(function (set) \
             if set == 'set' then a = 10+b; b = b+1 else return a end \
         end) \
         local x = assert(load(d)) \
         return x() == nil",
        true,
    );
    // single-upvalue main-chunk shape still receives globals so global
    // reads through `_ENV` keep working post-load.
    check_int(
        "local x = assert(load('return 1 + #_G')) return x() >= 1 and 1 or 0",
        1,
    );
}

#[test]
fn string_dump_header_is_per_version() {
    // calls.lua across 5.3/5.4/5.5 byte-checks the header prefix `string.dump`
    // produces. Drive a small dump through each dialect's VM and pluck the
    // version byte (offset 4) + LUAC_INT sanity word from the right slot.
    let cases = &[
        (LuaVersion::Lua53, 0x53u8, 0x11usize),
        (LuaVersion::Lua54, 0x54u8, 0x0fusize),
        (LuaVersion::Lua55, 0x55u8, 0usize), // 5.5 splits sanity per type; just check version
    ];
    for &(version, ver_byte, luac_int_off) in cases {
        let mut vm = Vm::new(version);
        let bytes = match vm.eval("return string.dump(function () return 7 end)") {
            Ok(vs) => match vs.into_iter().next() {
                Some(Value::Str(s)) => s.as_bytes().to_vec(),
                v => panic!("{version:?}: dump returned {v:?}, expected Str"),
            },
            Err(e) => panic!("{version:?}: eval failed: {e:?}"),
        };
        assert!(
            bytes.starts_with(b"\x1bLua"),
            "{version:?}: missing signature, got {:?}",
            &bytes[..4]
        );
        assert_eq!(bytes[4], ver_byte, "{version:?}: version byte mismatch");
        if luac_int_off > 0 {
            // 5.3 / 5.4 embed LUAC_INT = 0x5678 at a fixed offset. Reading 8 le
            // bytes there guards both the layout and the value choice — flipping
            // any earlier size byte would also offset this read.
            let off = luac_int_off;
            let int_bytes: [u8; 8] = bytes[off..off + 8].try_into().unwrap();
            assert_eq!(
                i64::from_le_bytes(int_bytes),
                0x5678,
                "{version:?}: LUAC_INT mismatch at offset {off}"
            );
        }
    }
}

#[test]
fn coroutine_basics() {
    // two-way value passing through resume/yield
    check_int(
        "local co = coroutine.create(function(a) local b = coroutine.yield(a+1) return b*10 end) \
         local _, x = coroutine.resume(co, 4) \
         local _, y = coroutine.resume(co, 7) \
         return x*100 + y",
        570, // x=5, y=70
    );
    // status transitions
    check_str(
        "local co = coroutine.create(function() coroutine.yield() end) \
         local a = coroutine.status(co) coroutine.resume(co) \
         local b = coroutine.status(co) coroutine.resume(co) \
         local c = coroutine.status(co) return a..','..b..','..c",
        b"suspended,suspended,dead",
    );
    // wrap: generator captured upvalues resolve to the creating thread's stack
    check_int(
        "local function gen(t) for _,v in ipairs(t) do coroutine.yield(v) end end \
         local data = {3, 4, 5} \
         local sum = 0 \
         for v in coroutine.wrap(function() gen(data) end) do sum = sum + v end \
         return sum",
        12,
    );
    // resuming a dead coroutine fails softly
    check_bool(
        "local co = coroutine.create(function() end) coroutine.resume(co) \
         local ok = coroutine.resume(co) return ok",
        false,
    );
    // an error inside the coroutine surfaces as (false, msg)
    check_str(
        "local co = coroutine.create(function() error('boom') end) \
         local ok, msg = coroutine.resume(co) \
         return tostring(ok)..':'..(msg:match('boom') or '?')",
        b"false:boom",
    );
    // yield outside a coroutine is an error
    check_error(
        "coroutine.yield(1)",
        "attempt to yield from outside a coroutine",
    );
    // running() reports the main thread
    check_bool("local _, m = coroutine.running() return m", true);

    // yield across a transparent native frame (P08 Stage 1): a chunk run by the
    // native `dofile` yields, then resume re-enters and continues the suspended
    // chunk frame to completion, its final return flowing back through dofile to
    // the resumer. Exercises call_value not truncating the suspended stack +
    // coro_continue restoring the frame's full register window.
    check_int(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')) \
         w:write('local x, z = coroutine.yield(10)\\n') \
         w:write('local y = coroutine.yield(20)\\n') \
         w:write('return x + y * z\\n') \
         w:close() \
         local co = coroutine.wrap(dofile) \
         local a = co(p) \
         local b = co(100, 101) \
         local c = co(7) \
         os.remove(p) \
         return a * 1000000 + b * 1000 + c",
        10_020_807,
    );

    // coroutine.close reports the error a coroutine died with, once: a thread
    // killed by error(100) closes to (false, 100), then to (true, nil).
    check_str(
        "local co = coroutine.create(error) \
         local _, m1 = coroutine.resume(co, 100) \
         local s2, m2 = coroutine.close(co) \
         local s3, m3 = coroutine.close(co) \
         return m1 .. ',' .. tostring(s2) .. ',' .. m2 .. ',' .. tostring(s3) .. ',' .. tostring(m3)",
        b"100,false,100,true,nil",
    );

    // coroutine.close runs the suspended coroutine's pending <close> handlers
    // (with nil error) and returns true; a handler that raises makes close
    // report (false, err).
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local trace = {} \
         local co = coroutine.create(function () \
           local a <close> = c(function (_, e) trace[#trace+1] = 'a:'..tostring(e) end) \
           coroutine.yield() \
         end) \
         coroutine.resume(co) \
         local ok = coroutine.close(co) \
         return tostring(ok) .. ',' .. table.concat(trace, ',') .. ',' .. coroutine.status(co)",
        b"true,a:nil,dead",
    );
    check_str(
        "local function c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.create(function () \
           local a <close> = c(function () error('boom') end) \
           coroutine.yield() \
         end) \
         coroutine.resume(co) \
         local ok, err = coroutine.close(co) \
         return tostring(ok) .. ',' .. (err:match('boom') or '?')",
        b"false,boom",
    );

    // yield across `pcall` (P08 Stage 2): a Lua function protected by pcall
    // yields, then resumes to completion; pcall wraps the final return as
    // (true, ...). Exercises the continuation frame surviving a yield.
    check_str(
        "local function f() coroutine.yield(10); return 20 end \
         local co = coroutine.create(function() return pcall(f) end) \
         local _, y1 = coroutine.resume(co) \
         local ok, st, v = coroutine.resume(co) \
         return y1 .. ',' .. tostring(ok) .. ',' .. tostring(st) .. ',' .. v",
        b"10,true,true,20",
    );
    // an error in the protected function after a yield is caught as (false, msg)
    check_str(
        "local function f() coroutine.yield(); error('boom') end \
         local co = coroutine.create(function() return pcall(f) end) \
         coroutine.resume(co) \
         local _, ok, msg = coroutine.resume(co) \
         return tostring(ok) .. ',' .. (msg:match('boom') or '?')",
        b"false,boom",
    );
    // coroutine.create(pcall): pcall itself is the body; the protected function
    // is passed on the first resume and may yield through pcall.
    check_str(
        "local co = coroutine.create(pcall) \
         local _, y = coroutine.resume(co, function() return coroutine.yield(5) + 1 end) \
         local ok, st, v = coroutine.resume(co, 100) \
         return y .. ',' .. tostring(ok) .. ',' .. tostring(st) .. ',' .. v",
        b"5,true,true,101",
    );
    // xpcall across a yield: the message handler runs on the post-yield error
    check_str(
        "local function f() coroutine.yield(); error('boom') end \
         local co = coroutine.create(function() return xpcall(f, function(m) return 'H:'..m end) end) \
         coroutine.resume(co) \
         local _, ok, msg = coroutine.resume(co) \
         return tostring(ok) .. ',' .. tostring(msg:match('^H:') ~= nil) .. ',' .. (msg:match('boom') or '?')",
        b"false,true,boom",
    );
}

#[test]
fn pcall_continuation_no_yield() {
    // success: results wrapped as (true, ...)
    check_str(
        "local ok, a, b = pcall(function() return 1, 2 end) \
         return tostring(ok) .. ',' .. a .. ',' .. b",
        b"true,1,2",
    );
    // error caught as (false, msg)
    check_str(
        "local ok, msg = pcall(function() error('x') end) \
         return tostring(ok) .. ',' .. (msg:match('x') or '?')",
        b"false,x",
    );
    // protected native function (no Lua frame pushed for it)
    check_str(
        "local ok, n = pcall(math.type, 1.0) return tostring(ok) .. ',' .. n",
        b"true,float",
    );
    // the caller's full register window survives an error caught by pcall
    // (the unwind truncates into it, then it is reinstated)
    check_int(
        "local function ce(f) local s = pcall(f); assert(not s) end \
         ce(function() error('e') end) \
         local a,b,c,d,e,f,g,h = 1,2,3,4,5,6,7,8 \
         return a+b+c+d+e+f+g+h",
        36,
    );
    // a <close> handler runs during the unwind, then pcall still catches
    check_str(
        "local function c(fn) return setmetatable({}, {__close = fn}) end \
         local seen \
         local ok, msg = pcall(function() \
           local x <close> = c(function(_, e) seen = e end) \
           error('boom') \
         end) \
         return tostring(ok) .. ',' .. (msg:match('boom') or '?') .. ',' .. (seen:match('boom') or '?')",
        b"false,boom,boom",
    );
    // the protected-call C-stack bound: self-recursive pcall terminates at a
    // bounded depth (~MAX_C_DEPTH) rather than running away to the Lua-stack
    // limit or overflowing the native stack
    check_bool(
        "local n = 0 \
         local function rec() n = n + 1; return pcall(rec) end \
         pcall(rec) \
         return n > 100 and n < 2000",
        true,
    );
    // xpcall: message handler transforms the error
    check_str(
        "local ok, m = xpcall(function() error('boom') end, function(msg) return 'H:'..msg end) \
         return tostring(ok) .. ',' .. tostring(m:match('^H:') ~= nil) .. ',' .. (m:match('boom') or '?')",
        b"false,true,boom",
    );
}

#[test]
fn yield_across_c_boundary() {
    // a thread is non-yieldable inside an unprotected C call (gsub replacement)
    check_str(
        "local r \
         coroutine.wrap(function() \
           string.gsub('a', 'a', function() r = coroutine.isyieldable() end) \
         end)() \
         return tostring(r)",
        b"false",
    );
    // yielding across that boundary errors rather than panicking; the sort is
    // itself a C call, so the yield surfaces as a protected-call failure
    check_str(
        "local co = coroutine.wrap(function() \
           local ok, msg = pcall(table.sort, {3, 1, 2}, coroutine.yield) \
           return tostring(ok) .. ',' .. (msg:match('C%-call boundary') or '?') \
         end) \
         return co()",
        b"false,C-call boundary",
    );
    // a coroutine closing itself (PUC 5.5): the to-be-closed handler runs and
    // the thread dies cleanly — resume yields (true) with no extra values, and
    // code after the close is unreachable
    check_str(
        "local c = function(f) return setmetatable({}, {__close = f}) end \
         local X = 'no' \
         local co = coroutine.create(function() \
           local v <close> = c(function() X = 'closed' end) \
           string.gsub('a', 'a', function() \
             coroutine.close() \
             X = 'unreachable' \
           end) \
         end) \
         local st, msg = coroutine.resume(co) \
         return tostring(st) .. ',' .. tostring(msg) .. ',' .. X .. ',' .. coroutine.status(co)",
        b"true,nil,closed,dead",
    );
    // if the self-close handler raises, the error becomes the coroutine's death
    // error (propagated past the protecting pcalls, not caught by them)
    check_str(
        "local c = function(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.create(function() \
           local v <close> = c(function() error('boom') end) \
           string.gsub('a', 'a', function() \
             assert(pcall(pcall, function() coroutine.close() end)) \
           end) \
         end) \
         local st, msg = coroutine.resume(co) \
         return tostring(st) .. ',' .. (msg:match('boom') or '?')",
        b"false,boom",
    );
    // a generic-for iterator, by contrast, is yieldable (it is called by the VM,
    // not via an unprotected C call): yielding through it suspends normally
    check_str(
        "local function iter(_, i) return coroutine.yield(i) end \
         local co = coroutine.wrap(function() \
           for i in iter, nil, 1 do end \
         end) \
         return tostring(co()) .. ',' .. tostring(co(7))",
        b"1,7",
    );
    // a chain of coroutines whose __close handlers each close the previous one
    // bottoms out as a (recoverable) "C stack overflow", not a panic
    check_str(
        "local coro = false \
         for i = 1, 1000 do \
           local previous = coro \
           coro = coroutine.create(function() \
             local cc <close> = setmetatable({}, {__close = function() \
               if previous then assert(coroutine.close(previous)) end \
             end}) \
             coroutine.yield() \
           end) \
           assert(coroutine.resume(coro)) \
         end \
         local st, msg = coroutine.close(coro) \
         return tostring(st) .. ',' .. (msg:match('C stack overflow') or '?')",
        b"false,C stack overflow",
    );
}

#[test]
fn named_vararg() {
    // a virtual named vararg reads like table.pack: integer key in range, "n"
    // count, else nil — including a float key with an integer value
    check_str(
        "local function f(...v) \
           return v[1]..','..v[2]..','..tostring(v.n)..','..tostring(v[5])..','..tostring(v[1.0]) end \
         return f(10, 20, 30)",
        b"10,20,3,nil,10",
    );
    // and it allocates nothing — PUC's `notab` "does not create any table"
    check_bool(
        "local function f(...v) return v[1] end \
         f(1, 2, 3) \
         collectgarbage() \
         local m = collectgarbage'count' \
         f(4, 5, 6); f(7, 8, 9) \
         return m == collectgarbage'count'",
        true,
    );
    // writing the named vararg materializes a real table; the write is then
    // visible through `...` (they share storage, PUC luaT_adjustvarargs)
    check_str(
        "local function aux(...t) t[1] = t[1] + 100; return ... end \
         return table.concat({aux(1, 2, 3)}, ',')",
        b"101,2,3",
    );
    // `...t` named `_ENV` makes the (materialized) vararg table the environment
    check_int(
        "local function aux(..._ENV) global x; x = 10; return x end return aux()",
        10,
    );
    // a named vararg captured by a nested closure escapes → materialized, still
    // correct as a table
    check_int(
        "local function f(...t) local g = function() return t.n end return g() end \
         return f(5, 6, 7, 8)",
        4,
    );
}

#[test]
fn call_metamethod_chains() {
    // a chain of __call tables resolves down to the real function
    check_int(
        "local function f() return 42 end \
         local t = setmetatable({}, {__call = f}) \
         t = setmetatable({}, {__call = t}) \
         t = setmetatable({}, {__call = t}) \
         return t()",
        42,
    );
    // 16 chained __call tables is one too many
    check_error(
        "local a = {} for i = 1, 16 do a = setmetatable({}, {__call = a}) end a()",
        "'__call' chain too long",
    );
    // a self-referential __call is caught the same way
    check_error(
        "local a = {} setmetatable(a, {__call = a}) a()",
        "'__call' chain too long",
    );
}

#[test]
fn load_reader_and_mode() {
    // function-reader form: pieces are concatenated until nil
    check_int(
        "local parts = {'return ', '1 + ', '2'} local i = 0 \
         local f = load(function() i = i + 1 return parts[i] end) return f()",
        3,
    );
    // a reader error is a soft failure (nil, msg)
    check_bool("return (load(function() error('boom') end)) == nil", true);
    // a reader returning a non-string fails softly too
    check_bool("return (load(function() return true end)) == nil", true);
    // mode 'b' rejects a text chunk
    check_str(
        "local _, m = load('return 1', 'c', 'b') return m",
        b"attempt to load a text chunk (mode is 'b')",
    );
    // a dumped chunk round-trips through the reader form under mode 'b'
    check_int(
        "local d = string.dump(load('return 6*7')) return load(function() local s=d d=nil return s end, 'c', 'b')()",
        42,
    );
}

#[test]
fn length_border_on_sparse_power_of_two_keys() {
    // a table whose keys are powers of two (1,2,4,...,2^62) all live in the
    // hash part; the length operator must report a small border quickly, not
    // the huge one (PUC unbound_search's linear fallback), else `for i=1,#t`
    // would loop ~2^62 times.
    check_int(
        "local t = {} for i = 62, 0, -1 do t[2^i] = true end \
         local n = #t return (n == 2 or n == 4) and n or -1",
        2,
    );
    // collectgarbage mode switches report the previous mode
    check_str(
        "collectgarbage('incremental') \
         local a = collectgarbage('generational') \
         local b = collectgarbage('incremental') return a..','..b",
        b"incremental,generational",
    );
    // collectgarbage("param", name [,value]) round-trips pacing parameters:
    // setting returns the previous value, getting returns the current one.
    check_int(
        "local old = collectgarbage('param', 'pause', 100) \
         return collectgarbage('param', 'pause')",
        100,
    );
    check_int(
        "collectgarbage('param', 'stepmul', 250) \
         return collectgarbage('param', 'stepmul', 300)",
        250,
    );
}

#[test]
fn incremental_gc_step() {
    // an incremental ("step") cycle must terminate and actually free garbage:
    // build a pile, drop it, then sweep with the smallest budget until a cycle
    // completes — the loop must end (true is eventually returned) and the heap
    // must shrink below the pre-drop size.
    check_bool(
        "collectgarbage('incremental') collectgarbage() \
         local a = {} for i = 1, 500 do a[i] = {{}} end \
         local before = collectgarbage('count') \
         a = nil \
         repeat until collectgarbage('step', 1) \
         return collectgarbage('count') < before",
        true,
    );
    // stepsize 0 = a single unbounded step that completes the whole cycle
    // (PUC "stop-the-world"): collectgarbage('step') returns true at once.
    check_bool(
        "collectgarbage('incremental') collectgarbage('param', 'stepsize', 0) \
         return collectgarbage('step')",
        true,
    );
    // generational mode: a "step" is a minor (full atomic) collection, so a
    // weak value created since the previous step is cleared immediately.
    // Regression for gengc.lua:122.
    check_bool(
        "collectgarbage('generational') \
         local t = setmetatable({}, {__mode = 'v'}) \
         t[1] = {10} \
         collectgarbage('step') \
         local r = t[1] == nil \
         collectgarbage('incremental') return r",
        true,
    );
}

#[test]
fn stdlibs_registered_in_package_loaded() {
    // every standard library must appear in package.loaded — nextvar.lua's
    // "clear globals" test deletes any global not present there, so a missing
    // entry (coroutine was missing) gets the library wiped mid-run.
    check_bool(
        "for _, n in ipairs{'string','table','math','os','io','utf8','debug','coroutine'} do \
           if package.loaded[n] ~= _G[n] then return false end \
         end \
         return true",
        true,
    );
}

#[test]
fn io_read_number_format() {
    // file:read("n") parses full Lua numerals (i64 precision, hex floats) and
    // pushes back the terminator so the next read sees it (PUC ungetc).
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')) \
         f:write(math.maxinteger, '\\n', '0xABCp-3\\n', '1234x') \
         f:close() \
         local g = assert(io.open(p, 'r')) \
         local a, b, c = g:read('n'), g:read('n'), g:read('n') \
         local d = g:read(1)  \
         g:close(); os.remove(p) \
         return a == math.maxinteger and b == 0xABCp-3 and c == 1234 and d == 'x'",
        true,
    );
    // load only accepts 'b'/'t' mode chars; 'B' (C-only fixed buffer) is rejected
    check_bool(
        "return (not pcall(load, '', '', 'B')) and load('return 7', 'n', 't')() == 7",
        true,
    );
    // read("n") stops at a stray second exponent marker: "234e+13E" parses
    // 234e13 and leaves 'E' for the next read (PUC read_number grammar).
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); f:write('234e+13E'); f:close() \
         local g = assert(io.open(p, 'r')) \
         local n, rest = g:read('n'), g:read(1) \
         g:close(); os.remove(p) \
         return n == 234e13 and rest == 'E'",
        true,
    );
    // an over-long numeral (>200 chars) fails to parse and leaves its tail in
    // the stream (PUC L_MAXLENNUM); read(0) reports EOF as nil, data as "".
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); f:write('1234'); for _=1,1000 do f:write('0') end; f:close() \
         local g = assert(io.open(p, 'r')) \
         local n = g:read('n') \
         local tail = g:read('a') \
         local eof = g:read(0) \
         g:close(); os.remove(p) \
         return n == nil and tail:match('^00*$') ~= nil and eof == nil",
        true,
    );
}

#[test]
fn io_error_and_close_semantics() {
    // an exhausted io.lines iterator closes its owned file, and calling it again
    // errors "file is already closed" (PUC io_readline).
    check_bool(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')); w:write('a\\nb\\n'); w:close() \
         local it = io.lines(p) \
         while it() do end \
         local ok, err = pcall(it) \
         os.remove(p) \
         return not ok and string.find(err, 'file is already closed', 1, true) ~= nil",
        true,
    );
    // closing an already-closed handle errors via the closed-file check.
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')); assert(f:close()) \
         local ok, err = pcall(io.close, f) \
         os.remove(p) \
         return not ok and string.find(err, 'closed file', 1, true) ~= nil",
        true,
    );
    // io.read / io.write on a closed default stream are usage errors; io.flush
    // exists and flushes the default output.
    check_bool(
        "local p = os.tmpname() \
         io.output(p); io.write('x'); assert(io.flush()) \
         io.input(p); io.close(io.input()) \
         local rok, rerr = pcall(io.read) \
         io.close(io.output()) \
         local wok, werr = pcall(io.write, 'y') \
         os.remove(p) \
         return not rok and string.find(rerr, 'input file is closed', 1, true) ~= nil \
            and not wok and string.find(werr, 'output file is closed', 1, true) ~= nil",
        true,
    );
    // io.lines with more than 250 read formats is rejected (PUC MAXARGLINE).
    check_bool(
        "local p = os.tmpname() \
         local w = assert(io.open(p, 'w')); w:write('hello\\n'); w:close() \
         local t = {}; for i = 1, 251 do t[i] = 1 end \
         local ok, err = pcall(io.lines, p, table.unpack(t)) \
         os.remove(p) \
         return not ok and string.find(err, 'too many arguments', 1, true) ~= nil",
        true,
    );
}

#[test]
fn io_open_read_write_seek() {
    // round-trip a real file through io.open: write, line/all reads, seek,
    // append, and os.remove. Exercises the FILE* handle methods end-to-end.
    check_bool(
        "local p = os.tmpname() \
         local f = assert(io.open(p, 'w')) \
         f:write('a\\n', 'bb\\n', 'ccc'):close() \
         local g = assert(io.open(p, 'r')) \
         local l1 = g:read('l')          \
         local pos = g:seek()            \
         local rest = g:read('a')        \
         g:close() \
         local h = assert(io.open(p, 'a')); h:write('Z'); local e = h:seek('end'); h:close() \
         local ok_remove = os.remove(p) \
         return l1 == 'a' and pos == 2 and rest == 'bb\\nccc' \
                and e == 9 and ok_remove == true and io.open(p) == nil",
        true,
    );
    // invalid open modes are rejected (PUC l_checkmode)
    check_bool(
        "for _, m in ipairs{'rw', 'rb+', 'r+bk', '', '+', 'b'} do \
           if pcall(io.open, 'x', m) then return false end \
         end \
         return true",
        true,
    );
}

#[test]
fn io_file_model_foundation() {
    // FILE* metatable, io.type, default streams, and close semantics.
    check_bool(
        "return io.type(io.stdin) == 'file' \
           and io.type(8) == nil and io.type({}) == nil \
           and getmetatable(io.stdin).__name == 'FILE*' \
           and io.input() == io.stdin and io.output() == io.stdout \
           and (not io.close(io.stdin)) and (not io.stdout:close()) \
           and tostring(io.stdout):sub(1, 5) == 'file '",
        true,
    );
    // calling the close method with no self argument errors with "got no value"
    check_bool(
        "local ok, err = pcall(io.stdin.close) \
         return not ok and string.find(err, 'got no value', 1, true) ~= nil",
        true,
    );
}

#[test]
fn io_std_streams_are_userdata() {
    // io.stdin/stdout/stderr are real FILE* userdata: distinct identity, the
    // "userdata" type, a non-null %p, usable as table keys, and rawlen errors
    // on them (events.lua:196). No placeholder table would satisfy all of these.
    check_bool(
        "return type(io.stdin) == 'userdata' \
           and io.stdin == io.stdin and io.stdin ~= io.stdout \
           and string.format('%p', io.stdin) ~= '(null)' \
           and ({[io.stdin] = 7})[io.stdin] == 7 \
           and not pcall(rawlen, io.stdin)",
        true,
    );
}

#[test]
fn gc_finalizers() {
    // __gc runs when a finalizable object is collected.
    check_bool(
        "local finished = false \
         local u = setmetatable({}, {__gc = function () finished = true end}) \
         u = nil \
         collectgarbage() \
         return finished",
        true,
    );
    // the collector is not reentrant: collectgarbage() inside a finalizer
    // returns fail (nil). Regression for gc.lua:698.
    check_bool(
        "local res = true \
         setmetatable({}, {__gc = function () res = collectgarbage() end}) \
         collectgarbage() \
         return res == nil",
        true,
    );
    // adding __gc to a metatable *after* setmetatable does not register the
    // object for finalization (PUC luaC_checkfinalizer is at setmetatable time).
    check_bool(
        "local ran = false \
         local mt = {} \
         local u = setmetatable({}, mt) \
         mt.__gc = function () ran = true end \
         u = nil \
         collectgarbage() \
         return not ran",
        true,
    );
    // db.lua :915: the finalizer's call frame must be tagged so
    // `debug.getinfo(1).namewhat == "metamethod"` and `.name == "__gc"`
    // (PUC marks ci with CIST_FIN). Without the tag, the test's
    // `repeat local a = {} until name` loop never exits.
    check_str(
        "local n = '' \
         setmetatable({}, {__gc = function () \
           local t = debug.getinfo(1) \
           n = t.namewhat .. ':' .. tostring(t.name) \
         end}) \
         collectgarbage() \
         return n",
        b"metamethod:__gc",
    );
    // Lua 5.5 reference manual §2.5.3: "An object can be marked again for
    // finalization by calling setmetatable with a different metatable, or
    // with the same metatable but with a different __gc field." Aliasing a
    // surviving handle across a finalize lets us re-register the same table
    // and have its `__gc` fire a second time. PUC `udata2finalize` clears
    // FINALIZEDBIT to allow the re-registration; the FIN-only guard on
    // `register_finalizable` mirrors that.
    check_int(
        "local count = 0 \
         local alive \
         local mt = {__gc = function (o) count = count + 1; alive = o end} \
         alive = setmetatable({}, mt) \
         alive = nil \
         collectgarbage() \
         setmetatable(alive, mt) \
         alive = nil \
         collectgarbage() \
         return count",
        2,
    );
}

#[test]
fn warn_library_5_4_plus() {
    // PUC 5.4+: warn defaults to off, `@on` enables, `@off` disables,
    // unknown `@<word>` ignored; multi-arg concatenates as one message.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval(
        "warn('silent before @on') \
         warn('@on') \
         warn('hello') \
         warn('multi', '-', 'arg') \
         warn('@unknown')  -- ignored, no emit \
         warn('@off') \
         warn('silent after @off')",
    )
    .expect("warn calls succeed");
    let log = vm.warn_log_take();
    let lines: Vec<String> = log
        .into_iter()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .collect();
    assert_eq!(lines, vec!["hello".to_string(), "multi-arg".to_string()]);
}

#[test]
fn warn_library_absent_on_5_3() {
    // PUC 5.3 has no `warn` in the base library. The global resolves to nil
    // and calling it should raise `attempt to call a nil value`.
    let mut vm = Vm::new(LuaVersion::Lua53);
    let result = vm.eval("warn('test')");
    match result {
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("attempt to call") || msg.contains("nil value"),
                "expected nil-call error, got: {msg}"
            );
        }
        Ok(_) => panic!("warn should be absent on 5.3 (no error)"),
    }
}

#[test]
fn os_execute_shell_probe_and_command() {
    // 5.5: no-arg `os.execute()` returns true (shell available); 5.1 returns 1.
    check_bool("return os.execute() == true", true);
    let mut vm51 = Vm::new(LuaVersion::Lua51);
    let v = vm51.eval("return os.execute()").expect("5.1 probe ok");
    assert_eq!(v.len(), 1);
    match v[0] {
        Value::Int(1) => {}
        v => panic!("5.1 os.execute() expected Int(1), got {v:?}"),
    }
    // 5.5: a real shell command. `(success, "exit", 0)` on success;
    // `(false, "exit", N)` on a non-zero exit. Build the assertion from
    // the triple so we exercise the full return shape.
    check_str(
        "local ok, kind, code = os.execute('true') \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"true:exit:0",
    );
    check_str(
        "local ok, kind, code = os.execute('exit 7') \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"false:exit:7",
    );
}

#[cfg(unix)]
#[test]
fn io_popen_read_write_and_close_status() {
    // Read pipe: capture child stdout, close returns the (success, "exit", 0)
    // triple — exactly what os.execute returns for the same command.
    check_str(
        "local f = io.popen('printf hello-popen') \
         local out = f:read('a') \
         local ok, kind, code = f:close() \
         return out..':'..tostring(ok)..':'..kind..':'..tostring(code)",
        b"hello-popen:true:exit:0",
    );
    // Write pipe: feed child stdin, then close — the shell's `cat > /dev/null`
    // exits 0. Just probe the close triple so the test stays deterministic
    // (no roundtrip).
    check_str(
        "local f = io.popen('cat > /dev/null', 'w') \
         f:write('whatever') \
         local ok, kind, code = f:close() \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"true:exit:0",
    );
    // Non-zero exit propagates into close's triple.
    check_str(
        "local f = io.popen('exit 4') \
         f:read('a') \
         local ok, kind, code = f:close() \
         return tostring(ok)..':'..kind..':'..tostring(code)",
        b"false:exit:4",
    );
}

#[test]
fn embedding_instr_budget_interrupts_infinite_loop() {
    // P09: a small budget catches a runaway loop. pcall captures the
    // raised "instruction budget exceeded" so the embedder gets control
    // back instead of the whole VM call propagating the error out.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_instr_budget(Some(5_000));
    let result = vm.eval(
        "local ok, err = pcall(function () \
           while true do end \
         end) \
         return ok, err",
    );
    let v = result.expect("pcall itself must succeed");
    assert_eq!(v.len(), 2);
    assert!(
        matches!(v[0], Value::Bool(false)),
        "pcall should have caught the budget error, got {:?}",
        v[0]
    );
    match v[1] {
        Value::Str(s) => assert!(
            s.as_bytes()
                .windows(20)
                .any(|w| w == b"instruction budget e"),
            "error msg should mention the budget: {:?}",
            String::from_utf8_lossy(s.as_bytes())
        ),
        v => panic!("expected error string, got {v:?}"),
    }
    // After tripping, the budget disarms so the embedder can resume.
    assert_eq!(vm.instr_budget_remaining(), None);
}

#[test]
fn embedding_instr_budget_unset_runs_normally() {
    // No budget set — the existing tests are proof, but pin it here
    // so a future change to the default doesn't silently break embedders
    // that never touch `set_instr_budget`.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval("local s = 0 for i=1,1000 do s = s + i end return s")
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(500_500)), "got {:?}", v[0]);
}

#[test]
fn embedding_new_minimal_has_no_globals() {
    // P09 sandbox: `new_minimal` leaves the globals table empty so the
    // embedder can choose exactly which libraries to expose. Probing for
    // `print` should raise "attempt to call a nil value".
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    let result = vm.eval("print('hi')");
    match result {
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("attempt to call") || msg.contains("nil"),
                "expected nil-call error, got: {msg}"
            );
        }
        Ok(_) => panic!("print should not exist on a new_minimal vm"),
    }
}

#[test]
fn embedding_selective_open_base_enables_print() {
    // P09: after `new_minimal`, `open_base` is enough to make `print`
    // and friends resolve. The host can keep math/io/debug/os out.
    let mut vm = Vm::new_minimal(LuaVersion::Lua55);
    vm.open_base();
    // `tostring` is part of the base library — exercising it confirms the
    // open ran. We can't directly observe `print` without intercepting
    // stdout, but `type(print)` works.
    let v = vm
        .eval("return type(print), type(tostring), tostring(42)")
        .unwrap();
    assert_eq!(v.len(), 3);
    assert!(matches!(v[0], Value::Str(_)));
    if let Value::Str(s) = v[0] {
        assert_eq!(s.as_bytes(), b"function");
    }
    if let Value::Str(s) = v[2] {
        assert_eq!(s.as_bytes(), b"42");
    }
    // math is *not* opened, so `math` is nil.
    let v = vm.eval("return math").unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Nil));
}

#[test]
fn embedding_memory_cap_catches_runaway_alloc() {
    // P09 soft cap: build a tight loop that allocates tables; the run loop
    // detects bytes > cap between dispatch turns, runs a collect, and
    // (still over) raises a catchable error. The cap path runs a full
    // collect before deciding to fire, so short-lived intermediates do
    // not trip — the inner loop must hold enough live state to push past
    // the post-collect threshold.
    //
    // v1.1 A1 Session C — luna-core's `Vm::new` defaults to
    // `NullJitBackend`; the interp loop ticks slow enough that GC has
    // breathing room between alloc bursts, masking the cap trip. To
    // exercise the same code path under interp-only, the inner loop
    // holds **all** allocated tables in a live array — no intermediate
    // gets reclaimed, so the cap fires on net live bytes rather than
    // on burst-vs-GC timing. Originally the test relied on Cranelift
    // packing allocations tight enough to outpace GC; the new shape
    // exercises the same `vm.bytes() > cap` predicate without that
    // timing dependency.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let baseline = vm.memory_used();
    vm.set_memory_cap(Some(baseline + 64 * 1024)); // small headroom
    let v = vm
        .eval(
            "local outer = {} \
             local ok, err = pcall(function () \
               for i = 1, 1000000 do outer[i] = string.rep('x', 100) end \
               return outer \
             end) \
             return ok, err",
        )
        .expect("pcall succeeds even when the inner alloc trips the cap");
    assert_eq!(v.len(), 2);
    assert!(
        matches!(v[0], Value::Bool(false)),
        "pcall should catch the cap error, got {:?}",
        v[0]
    );
    match v[1] {
        Value::Str(s) => assert!(
            s.as_bytes().windows(15).any(|w| w == b"memory cap exce"),
            "msg should mention the cap: {:?}",
            String::from_utf8_lossy(s.as_bytes())
        ),
        v => panic!("expected error string, got {v:?}"),
    }
}

#[test]
fn embedding_kevy_shape_short_script_per_request() {
    // P09 script-host shape: a Redis-style server gets many short scripts from
    // clients. Each call re-arms the budget, evaluates, harvests the
    // result, and the same Vm continues for the next request — possibly
    // after the previous one tripped its budget. Pin the round-trip.
    let mut vm = Vm::new(LuaVersion::Lua55);

    // (1) Normal short script with a generous budget.
    vm.set_instr_budget(Some(10_000));
    let v = vm
        .eval("local s = 0 for i=1,100 do s = s + i end return s")
        .unwrap();
    assert!(matches!(v[0], Value::Int(5050)));
    // Budget consumed but not tripped; some remaining.
    assert!(vm.instr_budget_remaining().unwrap_or(0) > 0);

    // (2) Trip the budget on the next request. The error propagates because
    // the embedder didn't wrap in pcall; the host catches it and continues.
    vm.set_instr_budget(Some(500));
    let err = vm.eval("while true do end").expect_err("budget must trip");
    let msg = vm.error_text(&err);
    assert!(msg.contains("instruction budget"), "got: {msg}");
    // After the trip the budget disarmed — important so a paranoid host
    // doesn't have to reset before EVERY eval.
    assert_eq!(vm.instr_budget_remaining(), None);

    // (3) Re-arm and run again — Vm state survived the budget trip cleanly.
    vm.set_instr_budget(Some(10_000));
    let v = vm
        .eval("local t = {1,2,3,4,5}; local s = 0 for _, x in ipairs(t) do s = s + x end return s")
        .unwrap();
    assert!(matches!(v[0], Value::Int(15)));

    // (4) Globals persist across requests — the host can pin shared state.
    vm.set_global("counter", Value::Int(0)).unwrap();
    for expected in 1..=5 {
        vm.set_instr_budget(Some(10_000));
        let v = vm.eval("counter = counter + 1; return counter").unwrap();
        assert!(
            matches!(v[0], Value::Int(n) if n == expected),
            "iter {expected}: got {:?}",
            v[0],
        );
    }
}

#[test]
fn embedding_memory_cap_unset_runs_normally() {
    // No cap = no enforcement. Allocates ~4MB of integer-keyed strings and
    // returns count to prove the loop ran to completion.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval(
            "local t = {} \
             for i = 1, 10000 do t[i] = tostring(i) end \
             return #t",
        )
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(10000)));
}

fn panic_string_native(
    _vm: &mut Vm,
    _fs: u32,
    _nargs: u32,
) -> Result<u32, luna_core::vm::LuaError> {
    panic!("boom from a native");
}

fn panic_static_str_native(
    _vm: &mut Vm,
    _fs: u32,
    _nargs: u32,
) -> Result<u32, luna_core::vm::LuaError> {
    panic!("static boom");
}

#[test]
fn embedding_native_panic_caught_as_lua_error() {
    // P09: a Rust panic inside a registered native must not unwind through
    // the dispatch loop. The catch_unwind in begin_call's native arm folds
    // it into a "native panic: <msg>" Lua error that pcall can catch.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f1 = vm.native(panic_string_native);
    vm.set_global("p1", f1).unwrap();
    let f2 = vm.native(panic_static_str_native);
    vm.set_global("p2", f2).unwrap();
    let v = vm.eval("return pcall(p1)").expect("pcall returns normally");
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        let msg = String::from_utf8_lossy(s.as_bytes());
        assert!(
            msg.contains("native panic") && msg.contains("boom from a native"),
            "string-payload panic should surface: {msg}"
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
    let v = vm.eval("return pcall(p2)").expect("pcall returns normally");
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        let msg = String::from_utf8_lossy(s.as_bytes());
        assert!(
            msg.contains("native panic") && msg.contains("static boom"),
            "static-str-payload panic should surface: {msg}"
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
}

#[test]
fn os_exit_is_callable_function() {
    // We can't actually call os.exit (it would tear the test process down),
    // but its absence in 5.1+ would surface as `not a function`. Probe via
    // `type(os.exit)` to confirm registration without invoking it.
    check_str("return type(os.exit)", b"function");
}

#[test]
fn warn_on_gc_error_5_4_plus() {
    // PUC 5.4+ `__gc` errors are routed through warn ("warn then continue"),
    // wrapped in `error in __gc metamethod (msg)`. No re-raise; the
    // collectgarbage call succeeds.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.eval(
        "warn('@on') \
         setmetatable({}, {__gc = function () error('@bang@') end}) \
         collectgarbage()",
    )
    .expect("collectgarbage swallows the __gc error under 5.4+");
    let log = vm.warn_log_take();
    assert_eq!(
        log.len(),
        1,
        "exactly one warn emission expected, got {log:?}"
    );
    let line = String::from_utf8_lossy(&log[0]);
    assert!(
        line.contains("error in __gc metamethod") && line.contains("@bang@"),
        "warn line should mention both the wrapper and the inner error: {line}"
    );
}

#[test]
fn xpcall_msgh_recursion_and_no_error_object() {
    // errors.lua :633: msgh that re-raises must be re-invoked with the new
    // error (PUC's `luaG_errormsg` leaves `L->errfunc` set across the msgh
    // call). With N=5 the chain bottoms out at err(0) → "END".
    check_str(
        "local function err (n) \
           if type(n) ~= 'number' then return n \
           elseif n == 0 then return 'END' \
           else error(n - 1) end \
         end \
         local _, msg = xpcall(error, err, 5) \
         return msg",
        b"END",
    );
    // errors.lua :637: at the soft cap (luna's `MSGH_CAP`) the unwind
    // synthesizes "C stack overflow" and re-invokes the msgh once more — the
    // string falls through err's non-number branch back to the outer xpcall.
    check_str(
        "local function err (n) \
           if type(n) ~= 'number' then return n \
           elseif n == 0 then return 'END' \
           else error(n - 1) end \
         end \
         local _, msg = xpcall(error, err, 300) \
         return msg",
        b"C stack overflow",
    );
    // errors.lua :606: an inner pcall(loop) inside an xpcall msgh sees the
    // stack-overflow as PUC's "error in error handling" (LUA_ERRERR) — the
    // `msgh_depth` scope routes `push_frame`'s overflow to that string.
    check_bool(
        "local function loop(x,y,z) return 1 + loop(x,y,z) end \
         local _, msg = xpcall(loop, function (m) \
           local _, e = pcall(loop) \
           return string.find(e, 'error handling') ~= nil \
         end) \
         return msg",
        true,
    );
    // errors.lua :648 / :668: a nil error object becomes "<no error object>"
    // at the unwind boundary (PUC `luaG_errormsg`). Covers both `error(nil)`
    // and `assert(nil, nil)` paths.
    check_str(
        "local _, m = pcall(function() error(nil) end); return m",
        b"<no error object>",
    );
    check_bool(
        "local _, m = pcall(assert, nil, nil); return type(m) == 'string'",
        true,
    );
    // errors.lua :672: `assert()` with no arguments raises the canonical
    // "bad argument #1 to 'assert' (value expected)" — PUC's luaL_checkany.
    check_bool(
        "local _, m = pcall(assert); return string.find(m, 'value expected') ~= nil",
        true,
    );
}

#[test]
fn io_buffered_writes_and_round_trip_time() {
    // files.lua :475: a write to a writable file is buffered in user space —
    // it succeeds against the buffer even when the underlying device would
    // refuse (the OS error surfaces at `:flush` instead, exactly like PUC
    // stdio). luna can't open `/dev/full` portably, so the check below covers
    // the part luna controls: the buffered write returns the file (not a
    // `(nil, msg)` triple) before any flush has happened.
    check_bool(
        "local f = assert(io.open(os.tmpname(), 'w')) \
         local r = f:write('abcd') \
         f:close() \
         return r == f",
        true,
    );
    // files.lua :302: a write to a read-only file is NOT buffered — the OS
    // surfaces EBADF and the call returns `(nil, msg, errno)`.
    check_bool(
        "local p = os.tmpname() \
         local fw = assert(io.open(p, 'w')); fw:write('x'); fw:close() \
         local f = assert(io.open(p, 'r')) \
         local a, b, c = f:write('xuxu') \
         f:close(); os.remove(p) \
         return not a and type(b) == 'string' and type(c) == 'number'",
        true,
    );
    // files.lua :847-:850: `os.time(os.date('*t', t))` round-trips a UTC
    // timestamp exactly. The calendar arithmetic is Hinnant's algorithm; the
    // `*t` table reads back through `os.time`'s normalizer.
    check_int(
        "local t = 1234567890 \
         return os.time(os.date('*t', t)) - t",
        0,
    );
    // files.lua :983: `os.time` normalizes table fields — `sec=-3602` carries
    // back through midnight to the previous month's last day.
    check_str(
        "local t1 = {year=2005, month=1, day=1, hour=1, min=0, sec=-3602} \
         os.time(t1) \
         return string.format('%d-%d-%d %d:%d:%d yday=%d', \
           t1.year, t1.month, t1.day, t1.hour, t1.min, t1.sec, t1.yday)",
        b"2004-12-31 23:59:58 yday=366",
    );
}

#[test]
fn parse_time_local_var_limit() {
    // errors.lua :775: more than MAXVARS=200 locals inside a function raises
    // the limit error at PARSE time so a later structural error (a missing
    // `end`) doesn't steal the spotlight. The "in function at line N" suffix
    // names the function's defining line.
    let s = std::iter::once("function foo ()\n  local ".to_string())
        .chain((1..=200).map(|j| format!("a{j}, ")))
        .chain(std::iter::once("b\nend".to_string()))
        .collect::<String>();
    let mut vm = Vm::new(LuaVersion::Lua55);
    let err = vm.load(s.as_bytes(), b"=t").unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("too many local variables"), "msg = {msg:?}");
    assert!(msg.contains("function at line 1"), "msg = {msg:?}");
    // Per-block scoping: the same names re-declared after each do…end stay
    // under the cap, so this large chunk must compile cleanly (locals.lua
    // exercises this pattern heavily).
    let s = (0..50)
        .map(|_| "do local a,b,c,d,e,f,g,h end ".to_string())
        .collect::<String>();
    let _ = vm.load(s.as_bytes(), b"=t").expect("scoped locals reset");
}

#[test]
fn debug_traceback_honours_level() {
    // db.lua :958: `debug.traceback(msg, level)` must enumerate from `level`,
    // not from the innermost frame. Skipping the top `level-1` frames cuts
    // the visible chain accordingly.
    check_int(
        "local function deep(lvl, n) \
           if lvl == 0 then return (debug.traceback('m', n)) end \
           return (deep(lvl-1, n)) \
         end \
         local function checkdeep(total, start) \
           local s = deep(total, start) \
           local rest = string.match(s, '^m\\nstack traceback:\\n(.*)$') \
           return select(2, string.gsub(rest, '\\n', '')) \
         end \
         return coroutine.wrap(checkdeep)(11, 5)",
        // 12 deep frames + 1 checkdeep, start=5 drops 4 → 9 frames → 8 newlines.
        8,
    );
}

#[test]
fn stripped_chunk_debug_surface() {
    // db.lua :992/:1004: stripped chunks render short_src as "?" (PUC `funcinfo`
    // substitutes "=?" when `Proto.source` is NULL; chunk_id strips the sigil).
    check_str(
        "local f = function () return 1 end \
         f = load(string.dump(f, true)) \
         return debug.getinfo(f).short_src",
        b"?",
    );
    // db.lua :993: `getinfo(level).currentline` is -1 in a stripped chunk
    // (PUC `getfuncline` returns -1 when per-instruction line info is absent).
    check_int(
        "local prog = 'return debug.getinfo(1).currentline' \
         local f = assert(load(string.dump(load(prog), true))) \
         return f()",
        -1,
    );
    // db.lua :984: `debug.getupvalue` returns "(no name)" when the upvalue
    // name was stripped (PUC `aux_upvalue` for a NULL name).
    check_str(
        "local a = 12 \
         local f = function () return a end \
         f = load(string.dump(f, true)) \
         local n = debug.getupvalue(f, 1) \
         return n",
        b"(no name)",
    );
    // db.lua :1030: a line hook installed before running a stripped chunk
    // still fires on the first instruction, but with `nil` as the line arg
    // (PUC pushes `currentline` only when `>= 0`).
    check_bool(
        "local foo = function () local a = 1; return a end \
         local s = load(string.dump(foo, true)) \
         local line = true \
         debug.sethook(function (e, l) line = l end, 'l') \
         s() \
         debug.sethook(nil) \
         return line == nil",
        true,
    );
}

#[test]
fn ephemeron_weak_key_tables() {
    // a chain of weak-key entries reachable through a root is fully retained
    // (ephemeron fixpoint: marking a value exposes the next key). gc.lua:336.
    check_int(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local x = nil \
         for i = 1, 50 do local n = {}; a[n] = {k = {x}}; x = n end \
         collectgarbage() \
         local n = x local i = 0 \
         while n do n = a[n].k[1]; i = i + 1 end \
         return i",
        50,
    );
    // once the root is dropped, the whole self-referential weak-key chain is
    // collected — no over-retention. gc.lua:340.
    check_bool(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local x = nil \
         for i = 1, 50 do local n = {}; a[n] = {k = {x}}; x = n end \
         x = nil \
         collectgarbage() \
         return next(a) == nil",
        true,
    );
}

#[test]
fn weak_table_string_keys_survive() {
    // strings are 'values' for weak tables (PUC `iscleared`): a string weak
    // key/value is never cleared and is resurrected by the collection, so the
    // entry survives even when no other reference to the string remains.
    // Regression for gc.lua:250.
    check_str(
        "local a = setmetatable({}, {__mode = 'k'}) \
         local s = 'weakkey-' .. tostring(98765) \
         a[s] = 'kept' \
         s = nil \
         collectgarbage() \
         return a['weakkey-98765']",
        b"kept",
    );
}

#[test]
fn weak_tables_clear_dead_entries() {
    // weak-value table: an entry whose value is otherwise unreachable is
    // cleared by a collection; a still-referenced value survives
    check_int(
        "local kept = {} local w = setmetatable({}, {__mode = 'v'}) \
         w.dead = {} w.live = kept \
         collectgarbage() \
         return (w.dead == nil and w.live == kept) and 1 or 0",
        1,
    );
    // weak-key table: an entry whose key is unreachable is dropped
    check_int(
        "local w = setmetatable({}, {__mode = 'k'}) \
         w[{}] = 1 local k = {} w[k] = 2 \
         collectgarbage() \
         local n = 0 for _ in pairs(w) do n = n + 1 end \
         return n",
        1, // only the entry keyed by the live 'k' remains
    );
    // a non-weak table keeps everything
    check_int(
        "local t = {} t[1] = {} collectgarbage() return t[1] ~= nil and 1 or 0",
        1,
    );
}

#[test]
fn repeat_until_with_captured_body_local_terminates() {
    // a closure capturing a repeat body's local forces a CLOSE on the loop-back
    // path; the close must sit only on that path, not inline between the
    // condition test and the back-jump (which would skip only the CLOSE on a
    // true condition and loop forever). Regression for closure.lua's
    // repeat/until-with-upvalue block.
    check_int(
        "local a = {} local i = 1 \
         repeat local x = i a[i] = function () return x end i = i + 1 \
         until i > 10 \
         return i",
        11,
    );
    // the condition itself may reference the captured body local
    check_int(
        "local a = {} local i = 1 \
         repeat local x = i a[i] = function () i = x + 1; return x end \
         until i > 10 or a[i]() ~= x \
         return (i == 11 and a[1]() == 1 and a[3]() == 3 and i == 4) and 1 or 0",
        1,
    );
}

#[test]
fn pattern_backref_zero_is_invalid() {
    // `%0` is not a valid back-reference: it must error, not panic on the
    // `d - b'1'` subtraction (debug-build overflow). Regression for pm.lua.
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval("return string.match('abc', '%0')") {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => {
            let msg = vm.error_text(&e);
            assert!(
                msg.contains("invalid capture index"),
                "unexpected error: {msg}"
            );
        }
    }
}

#[test]
fn string_format_modifiers() {
    // strings.lua regressions for string.format spec handling.
    // %s with a modifier rejects embedded zeros (PUC "string contains zeros").
    check_error(
        "return string.format('%10s', '\\0')",
        "string contains zeros",
    );
    // %a honours precision (round to N hex digits, ties-to-even).
    check_str("return string.format('%+.2A', 12)", b"+0X1.80P+3");
    check_str("return string.format('%.4A', -12)", b"-0X1.8000P+3");
    // `#` forces a radix point; the `0` flag zero-pads floats even with a
    // precision (unlike integer conversions).
    check_str("return string.format('%+#014.0f', 100)", b"+000000000100.");
    // per-conversion flag/width validation (PUC checkformat wording).
    check_error("return string.format('%100.3d', 10)", "invalid conversion");
    check_error("return string.format('%#i', 10)", "invalid conversion");
    check_error("return string.format('%010c', 10)", "invalid conversion");
    check_error("return string.format('%F', 10)", "invalid conversion");
    // over-long spec → "too long" (not "invalid conversion").
    check_error(
        "return string.format('%'..string.rep('0',600)..'d', 10)",
        "too long",
    );
}

#[test]
fn pushglobalfuncname_qualifies_nested_native_arg_error() {
    // errors.lua:381: `table.sort({1,2,3}, table.sort)` — the inner sort
    // (called as a comparator from the outer sort's native) detects bad
    // arg #1 (a number). PUC's `pushglobalfuncname` walks package.loaded
    // and qualifies the running function's name as `'table.sort'`.
    check_error("table.sort({1,2,3}, table.sort)", "'table.sort'");
    // errors.lua:382: `string.gsub('s', 's', setmetatable)` — the inner
    // setmetatable is invoked from gsub's native replacement loop; PUC
    // finds `_G.setmetatable` and strips the `_G.` prefix.
    check_error("string.gsub('s', 's', setmetatable)", "'setmetatable'");
    // A direct (non-nested) native arg error keeps the bare name.
    check_error("table.sort({}, 7)", "'sort'");
}

#[test]
fn syntax_error_source_uses_chunkid() {
    // errors.lua:402-416: a syntax error's source prefix is rendered via
    // luaO_chunkid (LUA_IDSIZE=60). `@file` is tail-truncated behind "...";
    // `=name` is head-truncated; a raw string source is wrapped as
    // `[string "first line..."]`. The prefix before the first `:` is ≤59.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let name_at = format!("@{}", "x".repeat(70));
    let src = format!("return load('x', '{name_at}')");
    let r = vm.eval(&src).expect("load itself succeeds");
    // `load` with a bad source (here: parse-time error) returns `(nil, msg)`;
    // the chunk's `return` surfaces both values.
    let msg = r.into_iter().nth(1).expect("(nil, msg)");
    if let crate::Value::Str(s) = msg {
        let bytes = s.as_bytes();
        assert!(
            bytes.starts_with(b"..."),
            "expected '...' truncation, got {:?}",
            String::from_utf8_lossy(bytes)
        );
        let prefix = bytes.split(|b| *b == b':').next().unwrap();
        assert!(prefix.len() <= 59, "prefix len {} > 59", prefix.len());
    } else {
        panic!("load did not return a string error");
    }
}

#[test]
fn light_userdata_from_debug_upvalueid() {
    // errors.lua:260: debug.upvalueid returns a light userdata, and
    // debug.setuservalue rejects it with "light userdata" in the message
    // (PUC's luaL_typeerror tag for LUA_TLIGHTUSERDATA).
    check_error(
        "local x = debug.upvalueid(function () return debug end, 1); \
         debug.setuservalue(x, {})",
        "light userdata",
    );
    // raw equality on identical light pointers
    check_str(
        "local f = function () return debug end; \
         local a, b = debug.upvalueid(f, 1), debug.upvalueid(f, 1); \
         return tostring(a == b)",
        b"true",
    );
    // type() collapses light userdata to "userdata" (PUC lua_typename)
    check_str(
        "return type(debug.upvalueid(function () return debug end, 1))",
        b"userdata",
    );
}

#[test]
fn table_concat_at_max_index() {
    // table.concat must not overflow `j + 1` when the range ends at maxi.
    // Regression for strings.lua:413.
    check_str(
        "return table.concat({[math.maxinteger] = 'alo'}, 'x', math.maxinteger, math.maxinteger)",
        b"alo",
    );
}

#[test]
fn error_message_fidelity() {
    // errors.lua regressions for PUC-faithful error wording.
    // A non-callable metamethod names the dispatching event.
    check_error(
        "local a = setmetatable({}, {__add = 34}); local _ = a + 1",
        "metamethod 'add'",
    );
    // A tail call to a nil field keeps the field name (frame popped early).
    check_error("local a = {}; return a.bbbb(3)", "field 'bbbb'");
    // __name (luaT_objtypename) drives type names in arithmetic/compare errors.
    check_error(
        "local x = setmetatable({}, {__name = 'My Type'}); local _ = x + 1",
        "on a My Type value",
    );
    check_error(
        "local x = setmetatable({}, {__name = 'My Type'}); local _ = x < x",
        "two My Type values",
    );
    // A field literally named `_ENV` is a field, not a global.
    check_error("local a = {_ENV = {}}; local _ = a._ENV.x + 1", "field 'x'");
    // collectgarbage rejects unknown options (luaL_checkoption).
    check_error("collectgarbage('nooption')", "invalid option");
    // A C function called as a method rewrites the self-argument error.
    check_error(
        "local t = setmetatable({}, {__index = string}); t:rep(2)",
        "calling 'rep' on bad self",
    );
    // luaL_optinteger position args report a proper argument error.
    check_error("return string.sub('a', {})", "number expected, got table");
    check_error("return string.sub('a', {})", "#2");
    // A stripped chunk (no source) reports the "?:?:" position prefix.
    check_error(
        "local f = assert(load(string.dump(function () return nil + 1 end, true))); f()",
        "?:?:",
    );
}

#[test]
fn getobjname_global_via_gettable() {
    // A global whose key constant index exceeds the GETFIELD operand limit is
    // compiled as GETTABLE; the operand must still name "global 'bbb'".
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&format!("aaa = x{i}; "));
    }
    src.push_str("local _ = bbb + 1");
    let mut vm = Vm::new(LuaVersion::Lua55);
    match vm.eval(&src) {
        Ok(v) => panic!("expected error, got {v:?}"),
        Err(e) => assert!(
            vm.error_text(&e).contains("global 'bbb'"),
            "unexpected: {}",
            vm.error_text(&e)
        ),
    }
}

#[test]
fn table_lib_metamethods() {
    // nextvar.lua: table.insert/remove/sort/concat honour __index/__newindex/__len.
    check_str(
        "local t = {}; local p = setmetatable({}, {__len = function () return #t end, \
         __index = t, __newindex = t}); for i = 1, 10 do table.insert(p, 1, i) end; \
         table.sort(p); return table.concat(p, ',')",
        b"1,2,3,4,5,6,7,8,9,10",
    );
    // table.insert with a maxinteger __len wraps to mininteger (must not hang).
    check_int(
        "local t = setmetatable({}, {__len = function () return math.maxinteger end}); \
         table.insert(t, 20); return (next(t))",
        i64::MIN,
    );
    // ipairs honours __index.
    check_int(
        "local a = setmetatable({n = 10}, {__index = function (t, k) \
         if k <= t.n then return k * 10 end end}); \
         local c = 0; for _ in ipairs(a) do c = c + 1 end; return c",
        10,
    );
    // pairs honours __pairs.
    check_int(
        "local function it(_, i) if i < 3 then return i + 1, (i + 1) * 10 end end; \
         local a = setmetatable({}, {__pairs = function (x) return it, x, 0 end}); \
         local c = 0; for _ in pairs(a) do c = c + 1 end; return c",
        3,
    );
}

#[test]
fn getinfo_names_c_boundary() {
    // locals.lua:514 — debug.getinfo of a synthetic C level names the native
    // from the call instruction that invoked it (e.g. "pcall").
    check_str(
        "local function f() local i = debug.getinfo(2); return i.namewhat .. '/' .. i.name end \
         return select(2, pcall(f))",
        b"global/pcall",
    );
}

#[test]
fn debug_getlocal_and_for_state() {
    // debug.getlocal returns the n-th active local (name, value).
    check_str(
        "local function basic(a, b) local c = a * b; local n, v = debug.getlocal(1, 3); \
         return n .. '=' .. v end \
         return basic(6, 7)",
        b"c=42",
    );
    // files.lua:447 — a generic-for loop's hidden control slots are named
    // "(for state)"; the 3rd is the to-be-closed value (PUC forlist).
    check_bool(
        "local function gettoclose(lv) lv = lv + 1; local st = 0 \
           for i = 1, 20 do local n, v = debug.getlocal(lv, i) \
             if n == '(for state)' then st = st + 1; if st == 3 then return v end end end end \
         local marker = setmetatable({}, {__close = function () end}) \
         local function iter(_, c) if c < 1 then return c + 1 end end \
         local got \
         for _ in iter, nil, 0, marker do got = gettoclose(1); break end \
         return got == marker",
        true,
    );
}

#[test]
fn close_handler_debug_parent_is_enclosing_function() {
    // locals.lua:288 — a __close handler on a normal exit runs within the
    // closing function's activation, so debug.getinfo(2) names that function
    // (PUC luaF_close; the handler is not a synthetic C boundary).
    check_str(
        "local captured \
         local function foo() \
           local _ <close> = setmetatable({}, {__close = function () \
             captured = debug.getinfo(2).name \
           end}) \
           return 1 \
         end \
         foo() \
         return captured",
        b"foo",
    );
}

#[test]
fn xpcall_traceback_sees_close_handler_frame() {
    // locals.lua:544 — debug.traceback called as xpcall msgh after a __close
    // handler raised must name the handler frame "metamethod 'close'" (PUC
    // luaG_errormsg runs msgh at the error point with stack intact). luna
    // snapshots the traceback at unwind entry so the catcher's msgh sees it.
    check_int(
        "local _, msg = xpcall(function () \
           local _ <close> = setmetatable({}, {__close = function () error('boom') end}) \
         end, debug.traceback) \
         return string.find(msg, \"in metamethod 'close'\") and 1 or 0",
        1,
    );
}

#[test]
fn non_closable_value_at_tbc_names_variable() {
    // locals.lua:554 — `local x <close> = {}` (no __close mm) errors with
    // "variable 'x' got a non-closable value (a table value)" (PUC
    // checkclosemth pulls the local name from the running frame's locvars).
    check_int(
        "local ok, msg = pcall(function () local x <close> = {} end) \
         return (not ok) and string.find(msg, \"variable 'x' got a non%-closable value\") and 1 or 0",
        1,
    );
}

#[test]
fn close_handler_removed_metamethod_errors() {
    // locals.lua:562 — __close was present at OP_TBC but cleared before close
    // time. luna's close_slots no longer silently skips: it raises
    // "attempt to call a <T> value (metamethod 'close')" (PUC
    // prepclosingmethod treats it as a non-callable target at close time).
    check_int(
        "local ok, msg = pcall(function () \
           local x <close> = setmetatable({}, {__close = print}) \
           getmetatable(x).__close = nil \
         end) \
         return (not ok) and string.find(msg, \"metamethod 'close'\") and 1 or 0",
        1,
    );
}

#[test]
fn stack_overflow_recovery_runs_close_in_errorh() {
    // locals.lua:659 — xpcall(overflow, errorh) where errorh sets up a
    // `<close>` local. The unwind restored the stack to the error-point
    // length (near MAX_LUA_STACK), so the next call_value_impl picked a
    // func_slot beyond the limit and re-overflowed. unwind now clamps the
    // restore to the catcher's caller window + MIN_STACK reserve.
    check_int(
        "local function overflow (n) overflow(n + 1) end \
         local function errorh (m) \
           local x <close> = setmetatable({}, {__close = function (o) o[1] = 42 end}) \
           return x \
         end \
         local _, obj = xpcall(overflow, errorh) \
         return obj[1]",
        42,
    );
}

#[test]
fn return_hook_for_native_names_callee() {
    // locals.lua:833 — a "return" hook firing after a native (debug.sethook)
    // returns must let getinfo(2) see the native, named via the caller's call
    // instruction ("sethook"). luna's run_hook pushes the hook with
    // `from_c = true` only when the hooked function was native, so dbg_frame
    // inserts a synthetic C level for it; for a Lua hooked function, `from_c`
    // is false and level 2 lands on that Lua frame.
    check_str(
        "local cap = '?' \
         local function hook (event) \
           if cap == '?' then cap = (debug.getinfo(2).name or '?') end \
         end \
         (function () debug.sethook(hook, 'r') end)() \
         debug.sethook() \
         return cap",
        b"sethook",
    );
}

#[test]
fn yield_inside_pairs_metamethod() {
    // nextvar.lua:953 — a coroutine.yield() inside a __pairs metamethod called
    // by pairs() must suspend cleanly (pairs drives __pairs as a continuation).
    check_int(
        "local t = setmetatable({10, 20, 30}, {__pairs = function (t) \
           local inc = coroutine.yield() \
           return function (t, i) if i > 1 then return i - inc, t[i - inc] end end, t, #t + 1 \
         end}) \
         local sum = 0 \
         local co = coroutine.wrap(function () for _, p in pairs(t) do sum = sum + p end end) \
         co(); co(1) \
         return sum",
        60,
    );
}

#[test]
fn numeric_for_coercion_and_bounds() {
    // nextvar.lua: numeric for coerces string bounds (PUC forprep tonumber).
    check_int(
        "local a = 0; for _ = '10', '1', '-2' do a = a + 1 end; return a",
        5,
    );
    // A float limit beyond the integer range gives an empty decreasing loop.
    check_int(
        "local c = 0; for _ = math.maxinteger, 10e100, -1 do c = c + 1 end; return c",
        0,
    );
    check_int(
        "local c = 0; for _ = math.mininteger, -10e100 do c = c + 1 end; return c",
        0,
    );
}

#[test]
fn multi_assign_snapshots_indexed_lhs() {
    // attrib.lua:505 — PUC manual §3.3.3: in `i, a[i], a, j, a[j], a[i+j] =
    // j, i, i, b, j, i`, every LHS index expression and reference must be
    // captured *before* any assignment. Without the snapshot, `a[i+j]` would
    // re-read `a` after `a = 1` and panic on "index a number".
    check_int(
        "local a, i, j, b = {'a', 'b'}, 1, 2; b = a \
         i, a[i], a, j, a[j], a[i+j] = j, i, i, b, j, i \
         assert(i == 2 and b[1] == 1 and a == 1 and j == b and b[2] == 2 and b[3] == 1) \
         return 1",
        1,
    );
    // Smaller PUC manual example: `i, a[i] = i+1, 20` writes a[old_i].
    check_int(
        "local a, i = {}, 3 \
         i, a[i] = i + 1, 20 \
         assert(i == 4 and a[3] == 20 and a[4] == nil) \
         return 1",
        1,
    );
}

#[test]
fn yieldable_close_at_block_exit() {
    // locals.lua:858 — a `do ... end` block's `<close>` may yield through its
    // __close handler; the block's OP_Close drives close handlers via the
    // interpreter loop, so a resume continues the close cleanly. Trace records
    // the order of body / close-enter / close-exit so we can detect a yield
    // that did not actually suspend.
    check_str(
        "local trace = {} \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.wrap(function () \
           do \
             local z <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'z1'; coroutine.yield('z'); trace[#trace + 1] = 'z2' \
             end) \
           end \
           trace[#trace + 1] = 'after' \
         end) \
         assert(co() == 'z') \
         co() \
         return table.concat(trace, ',')",
        b"z1,z2,after",
    );
}

#[test]
fn yieldable_close_at_function_return() {
    // locals.lua:874 — OP_Return's __close chain yields, then resumes to
    // deliver the original results to the caller (here, the `return x, X, 23`
    // pattern from locals.lua:277). The handler's `stack(10)` recursion is
    // the existing repro that shook out a self.top vs. abs_a + nret
    // off-by-one (post-close handler clobbered results).
    check_int(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local trace = {} \
         local co = coroutine.wrap(function () \
           local function foo (x) \
             local _ <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'y1'; coroutine.yield('y'); trace[#trace + 1] = 'y2' \
             end) \
             return x, 23 \
           end \
           local a, b = foo(1.5) \
           assert(a == 1.5 and b == 23) \
           trace[#trace + 1] = 'done' \
         end) \
         assert(co() == 'y') \
         co() \
         assert(trace[1] == 'y1' and trace[2] == 'y2' and trace[3] == 'done') \
         return 1",
        1,
    );
}

#[test]
fn yieldable_close_during_error_unwind() {
    // locals.lua :625..:1015 — `__close` handlers run during error unwind may
    // also yield; the Lua frame is popped before the close so `getinfo(2)`
    // names the C boundary (pcall), and `AfterClose::ResumeUnwind` defers
    // truncate + re-raise until every handler in the chain has run.
    check_str(
        "local trace = {} \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local co = coroutine.wrap(function () \
           local function foo () \
             local x <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'x1'; coroutine.yield('x'); trace[#trace + 1] = 'x2' \
             end) \
             local y <close> = f2c(function (_, msg) \
               trace[#trace + 1] = 'y1'; coroutine.yield('y'); trace[#trace + 1] = 'y2' \
             end) \
             error('boom') \
           end \
           local ok, msg = pcall(foo) \
           assert(not ok and msg:find('boom')) \
           trace[#trace + 1] = 'done' \
         end) \
         assert(co() == 'y') \
         assert(co() == 'x') \
         co() \
         return table.concat(trace, ',')",
        b"y1,y2,x1,x2,done",
    );
}

#[test]
fn close_handler_debug_parent_on_error_unwind_is_c_boundary() {
    // locals.lua :480 — during error unwind, `__close` handlers run after
    // their host Lua frame has been popped, so `getinfo(2).name` is the
    // outer caller (`pcall`), not the aborting function. Regressed once
    // when we deferred the frame pop until the close drained.
    check_str(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local got = '?' \
         local function foo () \
           local _ <close> = f2c(function (_, msg) \
             got = debug.getinfo(2).name or 'nil' \
           end) \
           error('boom') \
         end \
         pcall(foo) \
         return got",
        b"pcall",
    );
}

#[test]
fn goto_out_of_nested_for_closes_iterator_close_values() {
    // locals.lua :1219 — a `goto` leaving a generic-for loop must close the
    // iterator's implicit closing value (the 4th control slot). The body
    // block's reg_floor lands at `base + 4`, so the trampoline OP_Close must
    // target `base` (not `base + 4`) for the closing slot at `base + 3`.
    check_int(
        "local numopen = 0 \
         local function f2c(f) return setmetatable({}, {__close = f}) end \
         local function open (n) \
           numopen = numopen + 1 \
           return function () n = n - 1; if n > 0 then return n end end, \
                  nil, nil, \
                  f2c(function () numopen = numopen - 1 end) \
         end \
         local s = 0 \
         for i in open(3) do \
           for j in open(3) do \
             if i + j < 3 then goto endloop end \
             s = s + i \
           end \
         end \
         ::endloop:: \
         assert(numopen == 0, 'open iterators leaked: ' .. numopen) \
         return s",
        5,
    );
}

#[test]
fn dump_inherits_parent_source_in_child_protos() {
    // calls.lua :556 — PUC `DumpFunction` writes an empty source when a child
    // proto shares its parent's source, so a `<const>` string captured by N
    // child closures appears in the byte stream only twice (once in the
    // source text, once as the parent's constant), not 1 + 1 + N times.
    check_int(
        "local foo = load([[ \
           local str <const> = 'MARKER' \
           return { \
             function () return str end, \
             function () return str end, \
             function () return str end \
           } \
         ]]) \
         local dump = string.dump(foo) \
         local _, count = string.gsub(dump, 'MARKER', function () return 'X' end) \
         return count",
        2,
    );
}

#[test]
fn return_stat_caps_at_254_results() {
    // calls.lua :573 — OP_Return encodes `nret + 1` in a byte, so 255
    // explicit returns is a compile error; PUC reports "too many returns".
    check_str(
        "local code = 'return 10' .. string.rep(',10', 254) \
         local f, msg = load(code) \
         return tostring(f) .. '|' .. (msg:find('too many returns') and 'ok' or msg)",
        b"nil|ok",
    );
}

// ─── P09 api.lua-equivalent harness ───────────────────────────────────────
// PUC `api.lua` exercises the C-API contract (stack ops, call boundary,
// error propagation, metatable plumbing). luna's analogue is its Rust
// public surface; these tests pin the same semantic invariants there.

/// Native that pushes its arg count back as the result — analogous to
/// `lua_pushinteger(L, lua_gettop(L))`.
fn api_count_args(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(nargs as i64)]))
}

/// Native that returns each arg unchanged (multi-return passthrough).
fn api_echo(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
    let mut out = Vec::with_capacity(nargs as usize);
    for i in 0..nargs {
        out.push(vm.nat_arg(fs, nargs, i));
    }
    Ok(vm.nat_return(fs, &out))
}

#[test]
fn api_call_value_zero_args_zero_results() {
    // call_value with empty args, no return: equivalent to `lua_pcall(L, 0, 0)`.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"local x = 1 + 1", b"=chunk").expect("compile");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    assert!(r.is_empty(), "no-return chunk produces zero values: {r:?}");
}

#[test]
fn api_call_value_multi_arg_multi_result() {
    // The chunk consumes vararg, returns 3 values — host gets all three.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm
        .load(b"local a, b = ...; return a + b, a * b, a - b", b"=chunk")
        .expect("compile");
    let r = vm
        .call_value(Value::Closure(cl), &[Value::Int(3), Value::Int(4)])
        .expect("call");
    assert_eq!(r.len(), 3);
    assert!(matches!(r[0], Value::Int(7)), "sum: {:?}", r[0]);
    assert!(matches!(r[1], Value::Int(12)), "prod: {:?}", r[1]);
    assert!(matches!(r[2], Value::Int(-1)), "diff: {:?}", r[2]);
}

#[test]
fn api_native_sees_correct_nargs() {
    // PUC `lua_gettop` from a C function returns the arg count of the
    // call frame. luna's nargs parameter to `NativeFn` is the same.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_count_args);
    vm.set_global("count_args", f).unwrap();
    let v = vm
        .eval("return count_args(), count_args(1), count_args(1,2,3,4,5)")
        .unwrap();
    assert_eq!(v.len(), 3);
    assert!(matches!(v[0], Value::Int(0)));
    assert!(matches!(v[1], Value::Int(1)));
    assert!(matches!(v[2], Value::Int(5)));
}

#[test]
fn api_native_multi_return_passthrough() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_echo);
    vm.set_global("echo", f).unwrap();
    // echo(...) inside a vararg position spreads all results.
    let v = vm.eval("return echo('a','b','c')").unwrap();
    assert_eq!(v.len(), 3);
    for (got, want) in v.iter().zip([b"a", b"b", b"c"].iter()) {
        if let Value::Str(s) = got {
            assert_eq!(s.as_bytes(), *want);
        } else {
            panic!("not a string: {got:?}");
        }
    }
}

#[test]
fn api_globals_round_trip_through_set_and_lua_read() {
    // `Vm::set_global` from Rust must be visible to Lua, and a value Lua
    // stores into a global must be readable via the globals table.
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_global("from_host", Value::Int(42)).unwrap();
    let v = vm.eval("return from_host").unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(42)));
    vm.eval("from_lua = 'set by lua'").unwrap();
    let g = vm.globals();
    let key = Value::Str(vm.heap.intern(b"from_lua"));
    let got = g.get(key);
    if let Value::Str(s) = got {
        assert_eq!(s.as_bytes(), b"set by lua");
    } else {
        panic!("expected string, got {got:?}");
    }
}

#[test]
fn api_lua_error_propagates_to_host_with_render() {
    // `error("msg", 0)` raises the bare string; luna's `error_text`
    // renders it the same way PUC's `lua_tostring(L, -1)` would.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"error('boom', 0)", b"=chunk").expect("compile");
    let err = vm
        .call_value(Value::Closure(cl), &[])
        .expect_err("error chunk should fail");
    let text = vm.error_text(&err);
    assert_eq!(text, "boom");
}

#[test]
fn api_call_value_can_catch_internally() {
    // Lua's pcall returns (false, msg) and the host receives a clean Ok.
    let mut vm = Vm::new(LuaVersion::Lua55);
    let v = vm
        .eval(
            "local ok, msg = pcall(function () error('inner') end) \
             return ok, msg",
        )
        .unwrap();
    assert_eq!(v.len(), 2);
    assert!(matches!(v[0], Value::Bool(false)));
    if let Value::Str(s) = v[1] {
        // 5.5 prepends `chunkname:N:`; we just check the suffix.
        assert!(
            s.as_bytes().ends_with(b"inner"),
            "msg should end with 'inner': {:?}",
            String::from_utf8_lossy(s.as_bytes())
        );
    } else {
        panic!("expected error string, got {:?}", v[1]);
    }
}

#[test]
fn api_load_returns_callable_chunk() {
    // `Vm::load` returns a `Gc<LuaClosure>` the host can call repeatedly
    // (the chunk's compiled body is reusable, like `lua_load` produced
    // function on the stack).
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm.load(b"local n = ...; return n * n", b"=chunk").unwrap();
    let r1 = vm.call_value(Value::Closure(cl), &[Value::Int(5)]).unwrap();
    assert!(matches!(r1[0], Value::Int(25)));
    let r2 = vm.call_value(Value::Closure(cl), &[Value::Int(7)]).unwrap();
    assert!(matches!(r2[0], Value::Int(49)));
}

#[test]
fn api_native_runs_lua_callback_through_call_value() {
    // PUC C-API: `lua_call(L, n, m)` from inside a C function. luna's
    // analogue is `vm.call_value` from within a NativeFn. Native receives
    // a callback function as arg 1, calls it with 10, returns 1 + result.
    fn api_with_callback(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
        assert!(nargs >= 1);
        let cb = vm.nat_arg(fs, nargs, 0);
        let r = vm.call_value(cb, &[Value::Int(10)])?;
        let n = if let Value::Int(i) = r[0] {
            Value::Int(i + 1)
        } else {
            Value::Nil
        };
        Ok(vm.nat_return(fs, &[n]))
    }
    let mut vm = Vm::new(LuaVersion::Lua55);
    let f = vm.native(api_with_callback);
    vm.set_global("with_cb", f).unwrap();
    let v = vm
        .eval("return with_cb(function (x) return x * 3 end)")
        .unwrap();
    assert_eq!(v.len(), 1);
    assert!(matches!(v[0], Value::Int(31)), "got {:?}", v[0]);
}

#[test]
fn api_collect_garbage_returns_freed_count() {
    // PUC `lua_gc(L, LUA_GCCOLLECT, 0)` returns 0 (the previous "freed"
    // count). luna's `collect_garbage` returns the number of objects
    // freed in that pass. We can't pin an exact number across builds,
    // but it should be nonnegative and not panic under repeated calls.
    let mut vm = Vm::new(LuaVersion::Lua55);
    // make a bunch of garbage
    vm.eval("for i = 1, 100 do local t = {i, i} end").unwrap();
    let _ = vm.collect_garbage(); // returns usize ≥ 0 by type
    let _ = vm.collect_garbage(); // second call is a no-op-ish
}

#[test]
fn close_handler_debug_parent_is_lua_on_normal_close() {
    // PUC luaF_close: a normal (non-error) close handler runs *within* the
    // closing function's activation; getinfo(2).what must be "Lua", not "C".
    // Regressed once when begin_close passed `!error_close` for `from_c`.
    check_str(
        "local function f2c(f) return setmetatable({}, {__close = f}) end \
         local what = '?' \
         local function foo () \
           local _ <close> = f2c(function (_, msg) \
             what = debug.getinfo(2).what or 'nil' \
           end) \
           return 1 \
         end \
         foo() \
         return what",
        b"Lua",
    );
}
