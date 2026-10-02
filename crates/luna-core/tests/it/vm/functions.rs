//! Functions, closures, varargs, generic for and pcall.

use super::*;

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
