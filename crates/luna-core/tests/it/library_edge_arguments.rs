//! Library calls with arguments at the edge of their range, and debug
//! library access to state the libraries keep for themselves. Each case
//! used to panic (some only in debug builds, where integer overflow is
//! checked) or to abort on an allocation; the expected text is PUC's where
//! PUC gives one.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn eval_str(v: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(v);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{v:?}: snippet must return a string, got {other:?}"),
        },
        Err(e) => panic!("{v:?}: uncaught: {}", vm.error_text(&e)),
    }
}

/// `load` gives a chunk without upvalues a closure with a spare `_ENV`
/// cell; PUC's closure has none, so there is no upvalue 1 to read or set.
#[test]
fn loaded_function_without_upvalues() {
    let src = "local f = load(string.dump(function() return 1 end))
               return select('#', debug.getupvalue(f, 1)) .. ' '
                   .. select('#', debug.setupvalue(f, 1, 0)) .. ' ' .. f()";
    for v in &DIALECTS[1..] {
        assert_eq!(eval_str(*v, src), "0 0 1", "{v:?}");
    }
}

/// Library functions keep state in their upvalues (a coroutine, a file, a
/// state table); `debug.setupvalue` does not replace them.
#[test]
fn native_upvalues_are_not_replaced() {
    let src = "local it = string.gmatch('ab', '.')
               local w = coroutine.wrap(function() coroutine.yield(1) return 2 end)
               local lines = io.lines()
               local r = {}
               for _, f in ipairs({it, w, lines, require, os.setlocale, coroutine.resume}) do
                 r[#r + 1] = select('#', debug.setupvalue(f, 1, 0))
               end
               return table.concat(r, ' ') .. ' ' .. it() .. it() .. w() .. w()";
    for v in DIALECTS {
        assert_eq!(eval_str(v, src), "0 0 0 0 0 0 ab12", "{v:?}");
    }
}

/// The locale `os.setlocale` reports is not a table a script can reach
/// and change.
#[test]
fn setlocale_state_is_not_a_table() {
    let src = "local _, st = debug.getupvalue(os.setlocale, 1)
               return type(st) .. ' ' .. os.setlocale() .. ' ' .. os.setlocale('C', 'numeric')
                   .. ' ' .. os.setlocale(nil, 'time')";
    for v in &DIALECTS[1..] {
        assert_eq!(eval_str(*v, src), "string C C C", "{v:?}");
    }
}

#[test]
fn format_width_past_usize() {
    let src = "local ok, e = pcall(string.format, '%20000000000000000000d', 1) return e";
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            eval_str(v, src),
            "invalid conversion specification: '%20000000000000000000d'",
            "{v:?}"
        );
    }
}

/// PUC 5.1.5, 5.3.6 and 5.4.9 print `nil Invalid argument 22`.
#[test]
fn seek_from_current_by_mininteger() {
    let src = "local name = os.tmpname()
               local f = assert(io.open(name, 'w')) f:write('abc') f:close()
               f = assert(io.open(name))
               f:read(1)
               local a, b, c = f:seek('cur', math.mininteger or -2^63)
               f:close() os.remove(name)
               return tostring(a) .. ' ' .. b .. ' ' .. c";
    for v in DIALECTS {
        assert_eq!(eval_str(v, src), "nil Invalid argument 22", "{v:?}");
    }
}

#[test]
fn bit32_shift_by_mininteger() {
    let src = "return bit32.lshift(1, math.mininteger) .. ' ' .. bit32.rshift(1, math.mininteger)
               .. ' ' .. bit32.arshift(1, math.mininteger)";
    assert_eq!(eval_str(LuaVersion::Lua53, src), "0 0 0");
}

/// 5.1/5.2 sort on C `int` indices: the middle of `1 .. 2^31-1`.
/// PUC 5.2.4: "attempt to compare nil with number".
#[test]
fn sort_range_ending_at_int_max() {
    let src = "local t = setmetatable({1}, {__len = function() return 2^31 - 1 end})
               t[2^31 - 1] = 2
               local ok, e = pcall(table.sort, t, function(a, b) return a < b end)
               return e";
    assert!(eval_str(LuaVersion::Lua52, src).ends_with("attempt to compare nil with number"));
}

/// A `__len` far beyond the table's contents sizes nothing up front.
/// PUC 5.2.4 and 5.4.9: "attempt to compare two nil values".
#[test]
fn sort_with_a_huge_len() {
    let src = "local ok, e = pcall(table.sort,
                 setmetatable({}, {__len = function() return 2^31 - 2 end}))
               return e";
    for v in &DIALECTS[1..] {
        assert!(
            eval_str(*v, src).ends_with("attempt to compare two nil values"),
            "{v:?}"
        );
    }
}

/// A named vararg table's `n` asks for more values than the stack holds.
/// PUC 5.5.1: "stack overflow".
#[test]
fn vararg_count_beyond_the_stack() {
    let src = "local function f(...t) t.n = 1 << 29 return ... end
               local ok, e = pcall(f)
               return e";
    assert!(eval_str(LuaVersion::Lua55, src).ends_with("stack overflow"));
}

/// 5.1's traceback keeps the level in a C `int`.
#[test]
fn traceback_level_past_int() {
    let src = "local ok, t = pcall(debug.traceback, 'x', 2^63)
               return tostring(ok) .. ' ' .. t:sub(1, 18)";
    assert_eq!(eval_str(LuaVersion::Lua51, src), "true x\nstack traceback:");
}
