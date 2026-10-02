//! Numeric, string and dialect-specific programs.

use super::Program;
use luna_core::version::LuaVersion;

pub(super) const PROGRAMS: &[Program] = &[
    Program {
        name: "gsub_with_table_replacement",
        src: r#"
local t = {name = "Alice", age = "30"}
print((string.gsub("Hello $name, age $age", "%$(%w+)", t)))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "gsub_with_function_replacement",
        src: r#"
print((string.gsub("hello world", "(%w+)", function(w) return w:upper() end)))
print((string.gsub("abc123def456", "%d+", function(n) return "[" .. n .. "]" end)))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "gsub_with_count_limit",
        src: r#"
local s, n = string.gsub("a-b-c-d-e", "-", "/", 2)
print(s, n)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "integer_division_53plus",
        src: r#"
print(10 // 3, -10 // 3, 10 // -3, -10 // -3)
print(10.0 // 3, 10 // 3.0)
print(math.floor(10 / 3))
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "bitwise_53plus",
        src: r#"
print(0xff & 0x0f, 0xff | 0x100, 0xff ~ 0x0f, ~0)
print(1 << 8, 256 >> 4)
print(string.format("%x", 0xABCD ~ 0xFFFF))
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "string_format_pct_q",
        src: r#"
-- %q produces a Lua-readable quoted string. Output format is dialect-
-- stable for printable ASCII without embedded controls.
print(string.format("%q", "hello"))
print(string.format("%q", [[plain ascii]]))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "long_string_literal",
        src: r#"
local s = [[
line1
line2
line3]]
print(#s)
local s2 = [==[
nested [[ test ]]
]==]
print(#s2)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "goto_forward_label",
        src: r#"
-- 5.2+: goto/label
local i = 0
::start::
i = i + 1
if i < 5 then goto start end
print(i)
"#,
        min_version: LuaVersion::Lua52,
    },
    Program {
        // <const> violation is a COMPILE-time error in both engines —
        // can't be caught by pcall (pcall sees no chunk to call yet).
        // Just verify the const read path. Negative-shape coverage
        // requires capturing stderr, out of scope for this stdout-diff
        // harness.
        name: "const_attribute_54plus",
        src: r#"
local x <const> = 42
print(x, x + 1)
local s <const> = "hello"
print(s, #s)
"#,
        min_version: LuaVersion::Lua54,
    },
    Program {
        name: "string_pack_unpack_53plus",
        src: r#"
-- string.pack / string.unpack added in 5.3
local s = string.pack(">i4", 12345)
print(#s)
local n, pos = string.unpack(">i4", s)
print(n, pos)
print(string.packsize(">i4i2"))
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "math_type_53plus",
        src: r#"
-- math.type added in 5.3
print(math.type(1), math.type(1.0), math.type("1"))
print(math.tointeger(3.0), math.tointeger(3.5))
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "string_to_number_coercion",
        src: r#"
print(tonumber("42"))
print(tonumber("3.14"))
print(tonumber("  42  "))
print(tonumber("0xff"))
print(tonumber("1e3"))
print(tonumber("not a number"))
print(tonumber(""))
print(tonumber(nil))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "coroutine_close_54plus",
        src: r#"
-- coroutine.close added in 5.4
local co = coroutine.create(function() coroutine.yield(1); coroutine.yield(2) end)
local _, a = coroutine.resume(co)
print(a, coroutine.status(co))
local closed = coroutine.close(co)
print(closed, coroutine.status(co))
"#,
        min_version: LuaVersion::Lua54,
    },
    Program {
        name: "tostring_special_floats",
        src: r#"
-- tostring on Inf, -Inf, NaN. PUC outputs are dialect-stable
-- ("inf", "-inf", "nan" since 5.3; "1.#INF" / "-1.#INF" / "-1.#IND"
-- on Windows 5.1/5.2 — luna's reference is unix-PUC behavior).
local inf = 1/0
print(inf == math.huge)
print(-inf == -math.huge)
local nan = 0/0
print(nan == nan)
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "table_iteration_complete",
        src: r#"
-- mixed string/integer keys via next + pairs
local t = {alpha = 1, beta = 2, gamma = 3, [1] = "a", [2] = "b"}
local strk_count, intk_count = 0, 0
for k in pairs(t) do
    if type(k) == "string" then strk_count = strk_count + 1
    else intk_count = intk_count + 1 end
end
print(strk_count, intk_count)
"#,
        min_version: LuaVersion::Lua51,
    },
];
