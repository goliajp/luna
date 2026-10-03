//! Read-only tables (`Vm::set_readonly`, Redis's `lua_enablereadonlytable`):
//! every way a script, a library function or the host can write into a
//! read-only table raises "Attempt to modify a readonly table" and leaves
//! the table as it was; reads are unaffected; unmarking restores writes.
//! An assignment's error carries the position of the Lua code that made it,
//! one raised inside a library function carries none (`luaG_runerror`).
//! The JIT's compiled stores are covered in luna-jit's tests.

use luna_core::runtime::{Gc, Table, TableError, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

const MSG: &str = "Attempt to modify a readonly table";

fn table(vm: &mut Vm, expr: &str) -> Gc<Table> {
    match vm.eval(&format!("return {expr}")).expect("expr")[0] {
        Value::Table(t) => t,
        ref v => panic!("{expr} is {v:?}"),
    }
}

/// A Vm with all libraries and a read-only global `RO = {10, 20, 30, k = 1}`.
fn setup(v: LuaVersion) -> (Vm, Gc<Table>) {
    let mut vm = Vm::new(v);
    vm.eval("RO = {10, 20, 30, k = 1}").expect("setup");
    let ro = table(&mut vm, "RO");
    vm.set_readonly(ro, true);
    (vm, ro)
}

/// The error text of `src`, run as the chunk `user_script`.
fn err(vm: &mut Vm, src: &str) -> String {
    match vm.eval_chunk(src, "=user_script") {
        Ok(r) => panic!("{src} ran: {r:?}"),
        Err(e) => vm.error_text(&e),
    }
}

/// Run `src`, which returns one boolean, and assert it is true.
fn check(vm: &mut Vm, v: LuaVersion, src: &str) {
    match vm.eval_chunk(src, "=user_script") {
        Ok(r) => assert!(
            matches!(r[..], [Value::Bool(true)]),
            "{v:?}: {src} gave {r:?}"
        ),
        Err(e) => panic!("{v:?}: {src}: {}", vm.error_text(&e)),
    }
}

fn run(vm: &mut Vm, v: LuaVersion, src: &str) {
    if let Err(e) = vm.eval_chunk(src, "=user_script") {
        panic!("{v:?}: {src}: {}", vm.error_text(&e));
    }
}

/// `RO` still holds what [`setup`] put there.
fn unchanged(vm: &mut Vm, v: LuaVersion) {
    check(
        vm,
        v,
        "local n = 0 for _ in pairs(RO) do n = n + 1 end \
         return RO[1] == 10 and RO[2] == 20 and RO[3] == 30 and RO.k == 1 and n == 4 \
         and getmetatable(RO) == nil",
    );
}

#[test]
fn assignments_raise_with_the_position_of_the_lua_code() {
    let lines = [
        "RO.k = 2",              // SetField, existing key
        "RO.new = 1",            // SetField, new key
        "RO.k = nil",            // removing a key is a write too
        "RO[1] = 99",            // SetI, existing array slot
        "RO[4] = 40",            // SetI, past the array part
        "local i = 2 RO[i] = 0", // SetTable, integer register key
        "local k = 'k' RO[k] = 5",
        "RO[1.5] = 1",
        "RO[true] = 1",
        "local t = RO t.k = t.k + 1",
        "for i = 1, 3 do RO[i] = i end",
        "RO.absent = nil", // nothing would change; Redis refuses all the same
    ];
    for v in ALL {
        let (mut vm, _) = setup(v);
        for src in lines {
            assert_eq!(
                err(&mut vm, src),
                format!("user_script:1: {MSG}"),
                "{v:?}: {src}"
            );
            unchanged(&mut vm, v);
        }
        // the line of the assignment, inside a function
        let src = "local function f(t)\n  local x = 1\n  t.k = x\nend\nf(RO)";
        assert_eq!(err(&mut vm, src), format!("user_script:3: {MSG}"), "{v:?}");
        // inside a coroutine, and caught by pcall with the same text
        let src = "local co = coroutine.create(function() RO.k = 0 end)\n\
                   local ok, e = coroutine.resume(co)\n\
                   return not ok and e == 'user_script:1: ' .. MSG";
        check(&mut vm, v, &src.replace("MSG", &format!("'{MSG}'")));
        unchanged(&mut vm, v);
    }
}

#[test]
fn library_tables_and_globals_refuse_assignment() {
    for v in ALL {
        let mut vm = Vm::new(v);
        let g = vm.globals();
        let string_lib = table(&mut vm, "string");
        vm.set_readonly(string_lib, true);
        vm.set_readonly(g, true);
        for src in [
            "string.foo = 1",
            "string.len = nil",
            "x = 1",
            "print = nil",
            "_G.y = 2",
        ] {
            assert_eq!(
                err(&mut vm, src),
                format!("user_script:1: {MSG}"),
                "{v:?}: {src}"
            );
        }
        check(
            &mut vm,
            v,
            "return string.foo == nil and x == nil and y == nil and type(print) == 'function' \
             and string.len('abc') == 3",
        );
        // locals and other tables stay writable
        run(
            &mut vm,
            v,
            "local t = {} t.x = 1 t[1] = 2 rawset(t, 'y', 3) local s = string.upper('a')",
        );
    }
}

#[test]
fn natives_raise_without_a_position() {
    for v in ALL {
        let (mut vm, _) = setup(v);
        let mut cases = vec![
            "rawset(RO, 'k', 2)",
            "rawset(RO, 'new', 2)",
            "rawset(RO, nil, 1)", // read-only is checked before the key
            "setmetatable(RO, nil)",
            "setmetatable(RO, {})",
            "debug.setmetatable(RO, {})",
            "debug.setmetatable(RO, nil)",
            "table.insert(RO, 1)",
            "table.insert(RO, 1, 0)",
            "table.remove(RO)",
            "table.remove(RO, 1)",
            "table.sort(RO, function(a, b) return a > b end)",
        ];
        if v >= LuaVersion::Lua53 {
            cases.push("table.move({1, 2}, 1, 2, 1, RO)");
        }
        for src in cases {
            assert_eq!(err(&mut vm, src), MSG, "{v:?}: {src}");
            unchanged(&mut vm, v);
        }
        // a pcall'd write: the error value is the plain message
        check(
            &mut vm,
            v,
            &format!("return select(2, pcall(rawset, RO, 'k', 3)) == '{MSG}'"),
        );
        // reading from a read-only table into another one is fine
        if v >= LuaVersion::Lua53 {
            check(
                &mut vm,
                v,
                "local d = table.move(RO, 1, 3, 1, {}) return d[3] == 30",
            );
        }
    }
}

#[test]
fn table_sort_raises_only_when_it_would_store() {
    for v in ALL {
        let mut vm = Vm::new(v);
        run(
            &mut vm,
            v,
            "A = {1, 2} B = {1, 2, 3} C = {3, 1, 2, 5, 4} S = {'b', 'a'}",
        );
        for name in ["A", "B", "C", "S"] {
            let t = table(&mut vm, name);
            vm.set_readonly(t, true);
        }
        // PUC's quicksort stores nothing into an ordered array of up to three
        run(&mut vm, v, "table.sort(A) table.sort(B)");
        run(&mut vm, v, "table.sort(A, function(x, y) return x < y end)");
        assert_eq!(err(&mut vm, "table.sort(C)"), MSG, "{v:?}");
        assert_eq!(err(&mut vm, "table.sort(S)"), MSG, "{v:?}");
        check(
            &mut vm,
            v,
            "return table.concat(C, ',') == '3,1,2,5,4' and table.concat(S, ',') == 'b,a'",
        );
    }
}

#[test]
fn a_newindex_chain_reaching_a_read_only_table_raises() {
    for v in ALL {
        let (mut vm, _) = setup(v);
        // a read-only table's own __newindex is not called: it refuses first
        let ro2 = table(
            &mut vm,
            "setmetatable({}, {__newindex = function() CALLED = true end})",
        );
        vm.set_readonly(ro2, true);
        vm.set_global("RO2", Value::Table(ro2)).expect("global");
        assert_eq!(
            err(&mut vm, "RO2.x = 1"),
            format!("user_script:1: {MSG}"),
            "{v:?}"
        );
        assert_eq!(
            err(&mut vm, "RO2[1] = 1"),
            format!("user_script:1: {MSG}"),
            "{v:?}"
        );
        check(
            &mut vm,
            v,
            "return CALLED == nil and rawget(RO2, 'x') == nil",
        );
        // a writable proxy whose __newindex is the read-only table
        let src = "local p = setmetatable({}, {__newindex = RO})\np.x = 1";
        assert_eq!(err(&mut vm, src), format!("user_script:2: {MSG}"), "{v:?}");
        unchanged(&mut vm, v);
        // 5.3+ table.insert goes through __newindex, and reaches RO too
        if v >= LuaVersion::Lua53 {
            let src = "table.insert(setmetatable({}, {__newindex = RO}), 1)";
            assert_eq!(err(&mut vm, src), MSG, "{v:?}");
            unchanged(&mut vm, v);
        }
    }
}

#[test]
fn reads_and_non_writing_library_calls_still_work() {
    for v in ALL {
        let (mut vm, _) = setup(v);
        check(
            &mut vm,
            v,
            "local s = 0 for _, x in ipairs(RO) do s = s + x end \
             local n = 0 for _ in pairs(RO) do n = n + 1 end \
             return s == 60 and n == 4 and #RO == 3 and RO.k == 1 and rawget(RO, 2) == 20 \
             and table.concat(RO, ',') == '10,20,30' and next(RO) ~= nil",
        );
        // a read-only table still serves as an __index and as a metatable
        check(
            &mut vm,
            v,
            "local o = setmetatable({}, {__index = RO}) local a = o.k o.k = 5 \
             return a == 1 and o.k == 5 and rawget(RO, 'k') == 1",
        );
        check(
            &mut vm,
            v,
            "local o = setmetatable({}, RO) return getmetatable(o) == RO",
        );
    }
}

#[test]
fn table_setn_and_protected_metatables_keep_their_errors() {
    let (mut vm, _) = setup(LuaVersion::Lua51);
    // luaL_error names the Lua code that called the native
    assert_eq!(
        err(&mut vm, "table.setn(RO, 2)"),
        "user_script:1: 'setn' is obsolete"
    );
    for v in ALL {
        let mut vm = Vm::new(v);
        let t = table(&mut vm, "setmetatable({}, {__metatable = 'locked'})");
        vm.set_readonly(t, true);
        vm.set_global("P", Value::Table(t)).expect("global");
        // lbaselib checks __metatable before lua_setmetatable runs
        assert_eq!(
            err(&mut vm, "setmetatable(P, nil)"),
            "user_script:1: cannot change a protected metatable",
            "{v:?}"
        );
    }
}

#[test]
fn package_seeall_refuses_a_read_only_module() {
    let (mut vm, _) = setup(LuaVersion::Lua51);
    assert_eq!(err(&mut vm, "package.seeall(RO)"), MSG);
    unchanged(&mut vm, LuaVersion::Lua51);
}

#[test]
fn the_host_api_refuses_and_unmarking_restores_writes() {
    for v in ALL {
        let (mut vm, ro) = setup(v);
        let k = Value::Str(vm.intern_str("k"));
        // SAFETY: `ro` is a live table held by the global RO, and no other
        // reference into it exists during these calls
        let t = unsafe { ro.as_mut() };
        assert_eq!(
            t.set(&mut vm.heap, k, Value::Int(2)),
            Err(TableError::ReadOnly)
        );
        assert_eq!(
            t.set_int(&mut vm.heap, 1, Value::Int(2)),
            Err(TableError::ReadOnly)
        );
        assert!(!t.try_set_existing(k, Value::Int(2)));
        assert!(ro.is_readonly());
        let g = vm.globals();
        vm.set_readonly(g, true);
        let e = vm.set_global("z", 1_i64).expect_err("read-only globals");
        assert_eq!(vm.error_text(&e), MSG, "{v:?}");
        vm.set_readonly(g, false);
        vm.set_global("z", 1_i64).expect("writable again");
        unchanged(&mut vm, v);

        vm.set_readonly(ro, false);
        assert!(!ro.is_readonly());
        run(
            &mut vm,
            v,
            "RO.k = 2 RO[1] = 0 RO[4] = 4 rawset(RO, 'r', 1) table.insert(RO, 5) \
             table.sort(RO) setmetatable(RO, {}) setmetatable(RO, nil)",
        );
        check(
            &mut vm,
            v,
            "return RO.k == 2 and RO.r == 1 and #RO == 5 and RO[1] == 0 and RO[5] == 30",
        );
        // marked again, a key added while it was writable is refused too
        vm.set_readonly(ro, true);
        assert_eq!(
            err(&mut vm, "RO.r = 2"),
            format!("user_script:1: {MSG}"),
            "{v:?}"
        );
    }
}

#[test]
fn the_read_only_bit_and_the_metamethod_cache_leave_each_other_alone() {
    // the bit shares a word with the absent-metamethod bits a metatable
    // keeps: a lookup that misses sets one, a new key clears them
    for v in ALL {
        let mut vm = Vm::new(v);
        run(
            &mut vm,
            v,
            "MT = {} O = setmetatable({}, MT) for _ = 1, 3 do assert(O.x == nil) end",
        );
        let mt = table(&mut vm, "MT");
        vm.set_readonly(mt, true);
        // misses recorded while read-only keep it read-only
        run(&mut vm, v, "for _ = 1, 3 do assert(O.y == nil) O.z = 1 end");
        assert_eq!(
            err(&mut vm, "MT.__index = {x = 1}"),
            format!("user_script:1: {MSG}"),
            "{v:?}"
        );
        assert!(mt.is_readonly());
        vm.set_readonly(mt, false);
        // the new key drops the cached misses, and the bit stays clear
        check(&mut vm, v, "MT.__index = {x = 1} return O.x == 1");
        assert!(!mt.is_readonly());
        vm.set_readonly(mt, true);
        check(&mut vm, v, "return O.x == 1");
        assert_eq!(
            err(&mut vm, "MT.__index = nil"),
            format!("user_script:1: {MSG}"),
            "{v:?}"
        );
    }
}
