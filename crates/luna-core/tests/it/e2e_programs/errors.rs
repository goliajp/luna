//! Programs whose output depends on runtime error messages.

use super::Program;
use luna_core::version::LuaVersion;

pub(super) const PROGRAMS: &[Program] = &[
    Program {
        // `nil + 1` raises a PUC error whose message differs across
        // dialects (5.4 added variable-context like "local 'x'").
        // Check only that pcall caught it + "arithmetic" appears in msg.
        name: "err_arith_on_nil",
        src: r#"
local function bad() local x; return x + 1 end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "arithmetic") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_index_nil_field",
        src: r#"
-- accessing a field on nil
local function bad() local x; return x.field end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "nil") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_call_non_callable",
        src: r#"
-- calling a non-callable value
local function bad() local x = 42; return x() end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "call") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_concat_with_nil",
        src: r#"
local function bad() return "hello" .. nil end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "concatenate") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_compare_incompatible",
        src: r#"
-- comparing incompatible types
local function bad() return "abc" < 42 end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "compare") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_index_nil_with_string_key",
        src: r#"
local function bad()
    local t
    return t["key"]
end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "nil") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "err_divide_by_zero_int_53plus",
        src: r#"
-- 5.3+ integer division by zero raises; float / 0 returns Inf
local function bad() local x = 1; return x // 0 end
local ok, err = pcall(bad)
print(ok)
print(string.find(tostring(err), "zero") ~= nil)
-- float division by zero is NOT an error
print(1.0 / 0.0)  -- "inf"
print(-1.0 / 0.0)  -- "-inf"
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        // 5.3+ only — PUC 5.1/5.2 `luaB_assert` stringifies via
        // `luaL_error("%s", tostring(msg))`, dropping the table-ness.
        // PUC 5.3+ uses bare `lua_error()` and preserves the message
        // object. luna's assert always preserves (5.3+ semantics).
        name: "assert_with_table_message",
        src: r#"
local function bad() assert(false, {code = 7, msg = "fail"}) end
local ok, err = pcall(bad)
print(ok, type(err), err.code, err.msg)
"#,
        min_version: LuaVersion::Lua53,
    },
    Program {
        name: "error_with_level_arg",
        src: r#"
-- error(msg, 2) reports the CALLER's line, not the error() call's
local function inner() error("err-from-inner", 2) end
local function outer() inner() end
local ok, err = pcall(outer)
print(ok)
-- both engines should report a line in the same file context
print(string.find(tostring(err), "err-from-inner") ~= nil)
"#,
        min_version: LuaVersion::Lua51,
    },
];
