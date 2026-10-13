//! Unbounded nesting through every path that runs on the native stack ends
//! in the Lua error PUC raises (its C-call limit, or its Lua stack limit),
//! never in a crash: on embedder threads with 256 KB and 2 MB stacks
//! (`threads`) and on the main thread through the `luna` CLI (`cli`), in
//! all five dialects, with the interpreter alone, the method JIT, the
//! trace JIT and, when built in, the LLVM backend. The expected results
//! are what PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.9 / 5.5.1 return for the same
//! scripts; positions are written as `@` (the chunk names differ).

mod cli;
mod threads;

/// Each script starts with this: `norm(e)` turns everything up to the
/// last `chunk:line: ` into `@ `.
pub const PRELUDE: &str =
    r#"local function norm(e) return ((tostring(e):gsub("^.*:%d+: ", "@ "))) end "#;

pub struct Case {
    pub name: &'static str,
    /// first and last dialect the case applies to (`51` … `55`)
    pub dialects: (u8, u8),
    pub script: &'static str,
    pub expect: fn(u8) -> &'static str,
}

const ALL: (u8, u8) = (51, 55);

pub const CASES: &[Case] = &[
    Case {
        name: "lua_recursion",
        dialects: ALL,
        script: "local function f() return f() + 1 end local ok, e = pcall(f) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ stack overflow",
    },
    Case {
        name: "call_metamethod",
        dialects: ALL,
        script: "local t t = setmetatable({}, {__call = function(s) return (s()) end}) local ok, e = pcall(function() return t() end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ stack overflow",
    },
    Case {
        name: "index_metamethod",
        dialects: ALL,
        script: "local t t = setmetatable({}, {__index = function(t, k) return t[k] end}) local ok, e = pcall(function() return t.x end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "newindex_metamethod",
        dialects: ALL,
        script: "local t t = setmetatable({}, {__newindex = function(t, k, v) t[k] = v end}) local ok, e = pcall(function() t.x = 1 end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "eq_metamethod",
        dialects: ALL,
        script: "local mt = {} mt.__eq = function(a, b) return a == b end local a, b = setmetatable({}, mt), setmetatable({}, mt) local ok, e = pcall(function() return a == b end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "lt_le_metamethods",
        dialects: ALL,
        script: "local mt = {} mt.__lt = function(a, b) return a < b end mt.__le = function(a, b) return a <= b end local a, b = setmetatable({}, mt), setmetatable({}, mt) local ok1, e1 = pcall(function() return a < b end) local ok2, e2 = pcall(function() return a <= b end) return tostring(ok1) .. '|' .. norm(e1) .. '|' .. tostring(ok2) .. '|' .. norm(e2)",
        expect: |_| "false|@ C stack overflow|false|@ C stack overflow",
    },
    Case {
        name: "arith_concat_unm_metamethods",
        dialects: ALL,
        script: "local mt = {} mt.__add = function(a, b) return a + b end mt.__concat = function(a, b) return a .. b end mt.__unm = function(a) return -a end local a = setmetatable({}, mt) local r = {} for _, f in ipairs({function() return a + 1 end, function() return a .. 'x' end, function() return -a end}) do local ok, e = pcall(f) r[#r + 1] = tostring(ok) .. '|' .. norm(e) end return table.concat(r, '|')",
        expect: |_| "false|@ C stack overflow|false|@ C stack overflow|false|@ C stack overflow",
    },
    Case {
        name: "len_metamethod",
        dialects: (52, 55),
        script: "local mt = {} mt.__len = function(a) return #a end local a = setmetatable({}, mt) local ok, e = pcall(function() return #a end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "close_metamethod",
        dialects: (54, 55),
        script: "local function f() local x <close> = setmetatable({}, {__close = function() f() end}) end local ok, e = pcall(f) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "pairs_metamethod",
        dialects: (52, 55),
        script: "local t t = setmetatable({}, {__pairs = function(t) return pairs(t) end}) local ok, e = pcall(function() for k in pairs(t) do end end) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
    Case {
        name: "tostring_metamethod",
        dialects: ALL,
        script: "local mt = {} mt.__tostring = function(a) return tostring(a) end local ok, e = pcall(tostring, setmetatable({}, mt)) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
    Case {
        name: "sort_comparator",
        dialects: ALL,
        script: "local function f() table.sort({3, 2, 1}, function(a, b) f() return a < b end) end local ok, e = pcall(f) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
    Case {
        name: "gsub_replacement",
        dialects: ALL,
        script: "local function f() return (string.gsub('x', 'x', function() return f() end)) end local ok, e = pcall(f) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
    Case {
        name: "load_reader",
        dialects: ALL,
        script: "local function f() local s = false return load(function() if s then return nil end s = true f() return 'return 1' end) end local ok, e = pcall(f) return tostring(ok) .. '|' .. type(e)",
        expect: |_| "true|function",
    },
    Case {
        name: "coroutine_wrap",
        dialects: ALL,
        script: "local function f() return coroutine.wrap(f)() end local ok, e = pcall(f) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ C stack overflow",
    },
    Case {
        name: "coroutine_resume",
        dialects: ALL,
        script: "local last local function f() local ok, e = coroutine.resume(coroutine.create(f)) if not ok then last = e end end f() return norm(last)",
        expect: |_| "C stack overflow",
    },
    Case {
        name: "pcall_nesting",
        dialects: ALL,
        script: "local last local function f() local ok, e = pcall(f) if not ok then last = e end end f() return norm(last)",
        expect: |_| "C stack overflow",
    },
    Case {
        name: "xpcall_nesting",
        dialects: ALL,
        script: "local last local function h(m) return m end local function f() local ok, e = xpcall(f, h) if not ok then last = e end end f() return norm(last)",
        expect: |_| "C stack overflow",
    },
    Case {
        name: "handler_recursion",
        dialects: ALL,
        script: "local function h(m) return h(m) .. '' end local ok1, e1 = xpcall(error, h) local ok2, e2 = xpcall(error, function(m) error(m) end) return tostring(ok1) .. '|' .. norm(e1) .. '|' .. tostring(ok2) .. '|' .. norm(e2)",
        expect: |_| "false|error in error handling|false|error in error handling",
    },
    Case {
        name: "parser_nesting",
        dialects: ALL,
        script: "local f, e = (loadstring or load)('return ' .. string.rep('(', 100000) .. '1' .. string.rep(')', 100000)) return tostring(f) .. '|' .. norm(e)",
        expect: |v| match v {
            51 => "nil|@ chunk has too many syntax levels",
            52 | 53 => "nil|@ too many C levels (limit is 200) in main function near '('",
            _ => "nil|C stack overflow",
        },
    },
    Case {
        name: "self_recursion_without_end",
        dialects: ALL,
        script: "local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end for i = 1, 300 do f(10) end local ok, e = pcall(f, -1) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|@ stack overflow",
    },
    Case {
        name: "self_recursion_15000_deep",
        dialects: ALL,
        script: "local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end for i = 1, 300 do f(10) end local ok, e = pcall(f, 15000) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "true|15000",
    },
    Case {
        name: "self_recursion_150000_deep",
        dialects: ALL,
        script: "local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end for i = 1, 300 do f(10) end local ok, e = pcall(f, 150000) return tostring(ok) .. '|' .. norm(e)",
        // 5.1 allows 20000 nested calls (LUAI_MAXCALLS)
        expect: |v| {
            if v == 51 {
                "false|@ stack overflow"
            } else {
                "true|150000"
            }
        },
    },
    // the paths a native or the interpreter calls back through, each
    // nesting without bound: a comparator handing a JIT-compiled recursion
    // to the interpreter, a hook, `__close` run by `coroutine.close`,
    // `__index` run by `table.concat` and `__lt` run by `table.sort`
    Case {
        name: "sort_comparator_jit",
        dialects: ALL,
        script: "local function r(n) if n == 0 then return 0 end return 1 + r(n - 1) end for i = 1, 300 do r(10) end local a, b local t = {3, 2, 1} table.sort(t, function(x, y) if not a then a = select(2, pcall(r, 15000)) b = select(2, pcall(r, -1)) end return x < y end) return tostring(a) .. '|' .. norm(b)",
        expect: |_| "15000|@ stack overflow",
    },
    Case {
        name: "hook_callback",
        dialects: ALL,
        script: "local function f() table.sort({3, 2, 1}, function(a, b) f() return a < b end) end local ok, e debug.sethook(function() debug.sethook() ok, e = pcall(f) end, '', 1) local _ = 1 debug.sethook() return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
    Case {
        name: "close_from_native",
        dialects: (54, 55),
        script: "local last local function f() local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() f() end}) coroutine.yield() end) coroutine.resume(co) local ok, e = coroutine.close(co) if not ok then last = e end end local ok, e = pcall(f) return tostring(ok) .. '|' .. tostring(last ~= nil) .. '|' .. norm(e)",
        // the deepest close fails: at the C level limit, or with a small
        // thread stack where the native stack runs low first
        expect: |_| "true|true|nil",
    },
    Case {
        name: "index_from_native",
        dialects: ALL,
        script: "local t t = setmetatable({}, {__index = function(t, k) return table.concat(t, '', 1, 1) end}) local ok, e = pcall(table.concat, t, '', 1, 1) return tostring(ok) .. '|' .. norm(e)",
        // before 5.3 `table.concat` reads the table raw
        expect: |v| {
            if v < 53 {
                "false|invalid value (nil) at index 1 in table for 'concat'"
            } else {
                "false|C stack overflow"
            }
        },
    },
    Case {
        name: "lt_from_sort",
        dialects: ALL,
        script: "local mt = {} mt.__lt = function(a, b) table.sort({a, b}) return true end local ok, e = pcall(table.sort, {setmetatable({}, mt), setmetatable({}, mt)}) return tostring(ok) .. '|' .. norm(e)",
        expect: |_| "false|C stack overflow",
    },
];

pub fn case(name: &str) -> &'static Case {
    CASES.iter().find(|c| c.name == name).expect("case")
}
