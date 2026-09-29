//! Compiled comparisons have to respect the operand types. `==` and `<`
//! were lowered to a compare of the two raw payloads whatever the types
//! were, so integer 0 equalled nil (both payloads are 0), an integer equal
//! to a table's address equalled the table, two equal long strings (not
//! interned, so two objects) differed, `__eq` was never called for two
//! tables, and strings were ordered by address.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

#[derive(Clone, Copy, Debug)]
enum Tiers {
    Both,
    TraceOnly,
    MethodOnly,
}

fn run(version: LuaVersion, src: &str, jit: Option<(Tiers, u32)>) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    match jit {
        Some((tiers, hot)) => {
            vm.set_jit_enabled(!matches!(tiers, Tiers::TraceOnly));
            vm.set_trace_jit_enabled(!matches!(tiers, Tiers::MethodOnly));
            vm.jit.trace_hot_threshold = hot;
            vm.jit.call_hot_threshold = hot;
        }
        None => {
            vm.set_jit_enabled(false);
            vm.set_trace_jit_enabled(false);
        }
    }
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

/// Runs `src` under every tier and threshold and compares with the
/// interpreter; `traced` requires some trace to have run.
fn assert_same(versions: &[LuaVersion], src: &str, want: &str, traced: bool) {
    let mut bad = Vec::new();
    let mut dispatched = 0;
    for &v in versions {
        let (interp, _) = run(v, src, None);
        assert_eq!(interp, want, "{v:?}: interpreter");
        for tiers in [Tiers::Both, Tiers::TraceOnly, Tiers::MethodOnly] {
            for hot in [1, 2, 7] {
                let (jit, d) = run(v, src, Some((tiers, hot)));
                dispatched += d;
                if jit != interp {
                    bad.push(format!("{v:?} {tiers:?} hot {hot}: {jit}"));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "JIT differs from the interpreter:\n{}",
        bad.join("\n")
    );
    assert!(!traced || dispatched > 0, "no trace ran");
}

const MODERN: &[LuaVersion] = &[LuaVersion::Lua54, LuaVersion::Lua55];
const ALL: &[LuaVersion] = &[
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

#[test]
fn integer_zero_is_not_nil() {
    let src = r#"
        local function k1(x) local r = 0 for i = 0, 20, 2 do if x == nil then r = r + 1 end end return r end
        local function k2(x) local r = 0 for i = 1, 20 do if nil == x then r = r + 1 end end return r end
        local function k3(x) local r = 0 for i = 1, 20 do if x ~= nil then r = r + 1 end end return r end
        local function k4(x, y) local r = 0 for i = 1, 20 do if x == y then r = r + 1 end end return r end
        local out = {}
        for _ = 1, 3 do
          out[#out + 1] = k1(0) .. "/" .. k2(0) .. "/" .. k3(0) .. "/" .. k4(0, nil) .. "/" .. k4(nil, 0)
        end
        return table.concat(out, " ")"#;
    assert_same(MODERN, src, "0/0/20/0/0 0/0/20/0/0 0/0/20/0/0", true);
}

#[test]
fn integer_zero_is_not_nil_in_a_while_loop() {
    let src = r#"
        local function k(x) local r, i = 0, 0 while i < 20 do i = i + 1 if x == nil then r = r + 1 end end return r end
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(0) .. "/" .. k(nil) end
        return table.concat(out, " ")"#;
    assert_same(ALL, src, "0/20 0/20 0/20", true);
}

#[test]
fn integer_equal_to_a_table_address_is_not_the_table() {
    let src = r#"
        local t = {}
        local addr = tonumber(tostring(t):match("0x(%x+)"), 16)
        local function k(x, y) local r = 0 for i = 1, 20 do if x == y then r = r + 1 end end return r end
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(addr, t) .. "/" .. k(t, t) end
        return table.concat(out, " ")"#;
    assert_same(MODERN, src, "0/20 0/20 0/20", true);
}

#[test]
fn table_eq_metamethod_runs_after_plain_tables() {
    let src = r#"
        local calls = 0
        local mt = {__eq = function() calls = calls + 1 return true end}
        local function k(x, y) local r = 0 for i = 1, 20 do if x == y then r = r + 1 end end return r end
        local p, q = {}, {}
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(p, q) end
        out[#out + 1] = k(setmetatable({}, mt), setmetatable({}, mt))
        return table.concat(out, " ") .. " calls " .. calls"#;
    assert_same(MODERN, src, "0 0 0 20 calls 20", true);
}

#[test]
fn equal_long_strings_are_equal() {
    let src = r#"
        local function k(x, y) local r = 0 for i = 1, 20 do if x == y then r = r + 1 end end return r end
        local s1 = string.rep("a", 100)
        local s2 = string.rep("a", 99) .. "a"
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(s1, s2) .. "/" .. k("ab", "a" .. "b") .. "/" .. k(s1, "x") end
        return table.concat(out, " ")"#;
    assert_same(MODERN, src, "20/20/0 20/20/0 20/20/0", true);
}

#[test]
fn strings_order_by_contents() {
    let src = r#"
        local function k(x, y) local r = 0 for i = 1, 20 do if x < y then r = r + 1 end end return r end
        local function k2(x, y) local r = 0 for i = 1, 20 do if x <= y then r = r + 1 end end return r end
        local a, b = "b" .. string.rep("x", 50), "a" .. string.rep("x", 50)
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(a, b) .. "/" .. k(b, a) .. "/" .. k2(a, b) .. "/" .. k2(b, a) end
        return table.concat(out, " ")"#;
    assert_same(MODERN, src, "0/20/0/20 0/20/0/20 0/20/0/20", false);
}

#[test]
fn integer_zero_is_not_nil_in_loaded_bytecode() {
    // a dumped and reloaded chunk compares with a constant (`EqK`)
    let src = r#"
        local k = load(string.dump(function(x)
          local r = 0 for i = 1, 20 do if x == 0 then r = r + 1 end end return r
        end))
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(nil) .. "/" .. k(0) .. "/" .. k({}) end
        return table.concat(out, " ")"#;
    assert_same(MODERN, src, "0/20/0 0/20/0 0/20/0", true);
}

#[test]
fn strings_order_by_contents_in_a_while_loop() {
    let src = r#"
        local function k(x, y) local r, i = 0, 0 while i < 20 do i = i + 1 if x < y then r = r + 1 end end return r end
        local function k2(x, y) local r, i = 0, 0 while i < 20 do i = i + 1 if x <= y then r = r + 1 end end return r end
        local a, b = "b" .. string.rep("x", 50), "a" .. string.rep("x", 50)
        local out = {}
        for _ = 1, 3 do out[#out + 1] = k(a, b) .. "/" .. k(b, a) .. "/" .. k2(a, b) .. "/" .. k2(b, a) end
        return table.concat(out, " ")"#;
    assert_same(ALL, src, "0/20/0/20 0/20/0/20 0/20/0/20", false);
}
