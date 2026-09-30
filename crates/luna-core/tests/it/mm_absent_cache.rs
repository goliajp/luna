//! A metatable remembers which metamethods it lacks (one bit per event).
//! Each way a metatable can gain a field after a lookup missed must drop
//! that memory, and the table-valued `__index` walk must match the general
//! chain (depth limit, function and non-table links, errors).

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run(v: LuaVersion, src: &str) {
    let mut vm = Vm::new(v);
    if let Err(e) = vm.eval(src) {
        panic!("{v:?}: {}", vm.error_text(&e));
    }
}

fn run_all(src: &str) {
    for v in ALL {
        run(v, src);
    }
}

#[test]
fn index_added_after_a_miss_is_seen() {
    run_all(
        r#"
        local mt = {}
        local o = setmetatable({}, mt)
        for _ = 1, 3 do assert(o.x == nil) end
        mt.__index = {x = 1}
        assert(o.x == 1)
        "#,
    );
}

#[test]
fn index_revived_in_its_old_slot_is_seen() {
    run_all(
        r#"
        local mt = {__index = {x = 1}}
        local o = setmetatable({}, mt)
        assert(o.x == 1)
        mt.__index = nil
        assert(o.x == nil)
        mt.__index = {x = 2}
        assert(o.x == 2)
        "#,
    );
}

#[test]
fn rawset_and_growth_invalidate() {
    run_all(
        r#"
        local mt = {}
        local o = setmetatable({}, mt)
        assert(o.y == nil)
        rawset(mt, "__index", function(_, k) return k .. "!" end)
        assert(o.y == "y!")
        local mt2 = {}
        local p = setmetatable({}, mt2)
        assert(p.z == nil)
        for i = 1, 40 do mt2["k" .. i] = i end
        mt2.__index = {z = 3}
        assert(p.z == 3)
        "#,
    );
}

#[test]
fn newindex_added_after_a_miss_is_seen() {
    run_all(
        r#"
        local mt = {}
        local o = setmetatable({}, mt)
        o.a = 1
        mt.__newindex = function(t, k, v) rawset(t, k, v * 2) end
        o.b = 2
        assert(rawget(o, "b") == 4)
        "#,
    );
}

#[test]
fn arith_eq_len_added_after_a_miss_are_seen() {
    run_all(
        r#"
        local mt = {}
        local a, b = setmetatable({}, mt), setmetatable({}, mt)
        assert(not pcall(function() return a + 1 end))
        mt.__add = function() return 7 end
        assert(a + 1 == 7)
        assert(a ~= b)
        mt.__eq = function() return true end
        assert(a == b)
        "#,
    );
    for v in &ALL[1..] {
        run(
            *v,
            r#"
            local mt = {}
            local o = setmetatable({1, 2}, mt)
            assert(#o == 2)
            mt.__len = function() return 42 end
            assert(#o == 42)
            "#,
        );
    }
}

#[test]
fn string_metatable_gains_index_entries() {
    run_all(
        r#"
        local smt = getmetatable("")
        assert(("x").nope == nil)
        smt.__index.nope = function() return "yes" end
        assert(("x"):nope() == "yes")
        "#,
    );
}

#[test]
fn deep_and_mixed_chains_match_the_general_walk() {
    run_all(
        r#"
        local leaf = {k = "leaf"}
        local t = leaf
        for _ = 1, 7 do t = setmetatable({}, {__index = t}) end
        assert(t.k == "leaf" and t.missing == nil)
        local f = setmetatable({}, {__index = setmetatable({}, {__index = function(_, k) return k .. "?" end})})
        assert(f.q == "q?")
        local s = setmetatable({}, {__index = "abc"})
        assert(s.len == string.len)
        local u = setmetatable({}, {__index = setmetatable({}, {__index = 5})})
        assert(not pcall(function() return u.x end))
        "#,
    );
}

#[test]
fn index_loop_errors_as_before() {
    for v in ALL {
        let mut vm = Vm::new(v);
        let r = vm.eval(
            r#"
            local a, b = {}, {}
            setmetatable(a, {__index = b})
            setmetatable(b, {__index = a})
            return a.x
            "#,
        );
        let e = r.expect_err("a loop must error");
        let msg = vm.error_text(&e);
        let want = if v <= LuaVersion::Lua52 {
            "loop in gettable"
        } else {
            "'__index' chain too long; possible loop"
        };
        assert!(msg.contains(want), "{v:?}: {msg}");
    }
}
