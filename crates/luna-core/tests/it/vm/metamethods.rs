//! Metamethods and metatables.

use super::*;

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
    // bitwise operators do not convert strings from 5.4 on
    check_error(
        "return '12' & 4",
        "attempt to perform bitwise operation on a string value",
    );
    // 5.4+ (the default Vm dialect is 5.5) reports string-involved
    // arithmetic faults with lstrlib's per-op wording; dialect fixtures
    // 5.3/541 + 5.4/551 + 5.5/252 pin the full matrix.
    check_error(
        "return 'abc' + 1",
        "attempt to add a 'string' with a 'number'",
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
