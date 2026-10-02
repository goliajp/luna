//! Long-tail edge cases across the language and library.

use super::Program;
use luna_core::version::LuaVersion;

pub(super) const PROGRAMS: &[Program] = &[
    Program {
        name: "pcall_returns_multiple",
        src: r#"
local ok, a, b, c = pcall(function() return 1, 2, 3 end)
print(ok, a, b, c)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "select_varargs",
        src: r#"
local function f(...) return select('#', ...), select(2, ...) end
print(f('a', 'b', 'c', 'd'))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        // 5.1 cannot yield across a pcall boundary ("attempt to
        // yield across C-call boundary"); 5.2+ made pcall continuation-
        // aware. Requires 5.2+.
        name: "nested_coroutine_pcall",
        src: r#"
local function inner()
    coroutine.yield(10)
    coroutine.yield(20)
end
local co = coroutine.create(function()
    local ok, err = pcall(inner)
    print("pcall returned:", ok)
    coroutine.yield(99)
end)
local _, a = coroutine.resume(co)
local _, b = coroutine.resume(co)
local _, c = coroutine.resume(co)
print(a, b, c)
"#,
        min_version: LuaVersion::Lua52,
    },
    Program {
        name: "pattern_anchors_captures",
        src: r#"
-- anchors + multi-capture
print(string.match("abc123xyz", "^(%a+)(%d+)(%a+)$"))
-- alternation via char class
for w in string.gmatch("apple,banana;cherry", "[^,;]+") do print(w) end
-- pattern with %b balanced match
print(string.match("(foo(bar)baz)", "%b()"))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "table_insert_remove_mid",
        src: r#"
local t = {1, 2, 3, 4, 5}
table.insert(t, 3, 99)
print(table.concat(t, ","))
local removed = table.remove(t, 4)
print(removed, table.concat(t, ","))
table.insert(t, 100)
print(table.concat(t, ","))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "string_byte_char_format",
        src: r#"
print(string.byte("A"), string.byte("z"))
print(string.char(65, 66, 67))
print(string.format("%05d %.3f %s", 7, 3.14159, "hi"))
print(string.format("%x %X %o", 255, 255, 8))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        // 3-arg `string.rep(s, n, sep)` is a 5.2+ extension; PUC 5.1 ignores
        // the separator. Restrict to 5.2+.
        name: "string_reverse_rep_sub",
        src: r#"
print(string.reverse("hello"))
print(string.rep("ab", 4))
print(string.rep("x", 3, "-"))
print(string.sub("abcdefgh", 2, 5))
print(string.sub("abcdefgh", -3))
print(string.upper("Mixed Case 42"))
print(string.lower("Mixed Case 42"))
"#,
        min_version: LuaVersion::Lua52,
    },
    Program {
        name: "math_floor_modulo_bounds",
        src: r#"
print(math.floor(3.9), math.floor(-3.1), math.ceil(3.1), math.ceil(-3.9))
print(math.max(1, 5, 3, 7, 2))
print(math.min(1, 5, 3, 7, 2))
print(math.huge > 1e300)
print(math.huge == math.huge)
print(0/0 ~= 0/0)  -- NaN never equals itself
print(7 % 3, -7 % 3, 7 % -3, -7 % -3)  -- Lua modulo semantics
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "tostring_tonumber_edges",
        src: r#"
print(tostring(nil), tostring(true), tostring(false))
print(tonumber("42"), tonumber("3.14"))
print(tonumber("0x1f"), tonumber("not a number"))
print(tonumber("100", 2), tonumber("ff", 16), tonumber("777", 8))
print(type(tonumber("42")))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "error_object_propagation",
        src: r#"
-- error with non-string value (table)
local ok, err = pcall(function() error({code = 42, msg = "boom"}) end)
print(ok, type(err), err.code, err.msg)
-- error with nil
local ok2, err2 = pcall(function() error(nil) end)
print(ok2, type(err2))
-- assert with message
local ok3, err3 = pcall(function() assert(false, "assertion-message") end)
print(ok3, err3)
"#,
        min_version: LuaVersion::Lua51,
    },
    // `pcall + non-tail-call deep recursion` raises stack overflow. The
    // `1 +` blocks tail-call optimization so each call grows the value
    // stack until MAX_LUA_STACK fires (~250k frames, a few ms). luna
    // and PUC match. The tail-call form (`return f(n+1)`) is excluded —
    // both engines run it forever (TCO is correct Lua semantics; not a
    // bug). See docs/known-bugs/fixed/pcall-stack-overflow-investigation.md
    Program {
        name: "pcall_stack_overflow",
        src: r#"
local function f(n) return 1 + f(n + 1) end
local ok, err = pcall(f, 0)
print(ok)
print(string.find(tostring(err), "stack overflow") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "ipairs_stops_at_nil",
        src: r#"
-- ipairs is the integer-key iterator that stops at the first nil
local t = {10, 20, nil, 40, 50}
local count, sum = 0, 0
for i, v in ipairs(t) do count = count + 1; sum = sum + v end
print(count, sum)  -- expects 2, 30 (stops at nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "pairs_iteration_order_irrelevant",
        src: r#"
-- pairs iteration order is unspecified; sum the keys and values to
-- get a deterministic comparison cross-engine
local t = {x = 1, y = 2, z = 3, [4] = 4, [10] = 10}
local ksum, vsum = 0, 0
for k, v in pairs(t) do
    if type(k) == "number" then ksum = ksum + k end
    if type(v) == "number" then vsum = vsum + v end
end
print(ksum, vsum)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "string_len_byte_position",
        src: r#"
local s = "Hello, world!"
print(#s, string.len(s))
print(s:sub(1, 5), s:sub(-6, -1))
local b, e = string.find(s, "world")
print(b, e)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "table_sort_with_comparator",
        src: r#"
local t = {"banana", "apple", "cherry", "date"}
table.sort(t)
print(table.concat(t, "/"))
table.sort(t, function(a, b) return #a < #b end)
print(table.concat(t, "/"))
"#,
        min_version: LuaVersion::Lua51,
    },
];
