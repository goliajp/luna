//! P2-D B12 smoke tests: Lua newtype + LuaFunction + LuaTable + LuaRoot.

use luna_jit::version::LuaVersion;
use luna_jit::{Lua, LuaFunction, LuaTable};
use luna_core::runtime::Value;

#[test]
fn lua_new_eval_basic() {
    let mut lua = Lua::new();
    lua.open_base();
    let r: i64 = lua.eval("return 1 + 2").unwrap();
    assert_eq!(r, 3);
}

#[test]
fn lua_sandbox_builder() {
    let mut lua = Lua::sandbox(LuaVersion::Lua54)
        .open_base()
        .open_math()
        .with_instr_budget(1_000_000)
        .build();
    let r: f64 = lua.eval("return math.sqrt(16)").unwrap();
    assert!((r - 4.0).abs() < 1e-9);
}

#[test]
fn lua_set_global_primitive() {
    let mut lua = Lua::new();
    lua.open_base();
    lua.set_global("answer", 42_i64).unwrap();
    lua.set_global("name", "luna").unwrap();
    let n: i64 = lua.eval("return answer").unwrap();
    let s: String = lua.eval("return name").unwrap();
    assert_eq!(n, 42);
    assert_eq!(s, "luna");
}

#[test]
fn lua_create_function_and_call_from_script() {
    let mut lua = Lua::new();
    lua.open_base();
    let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
    lua.set_global("add", add).unwrap();
    let r: i64 = lua.eval("return add(40, 2)").unwrap();
    assert_eq!(r, 42);
}

#[test]
fn lua_function_call_from_host() {
    let mut lua = Lua::new();
    let f: LuaFunction = lua.create_function(|x: i64| -> i64 { x * 10 });
    let r: i64 = f.call(&mut lua, (7_i64,)).unwrap();
    assert_eq!(r, 70);
}

#[test]
fn lua_table_set_get() {
    let mut lua = Lua::new();
    lua.open_base();
    let t: LuaTable = lua.create_table();
    t.set(&mut lua, "key", 99_i64).unwrap();
    t.set(&mut lua, "name", "luna").unwrap();
    let n: i64 = t.get(&mut lua, "key").unwrap();
    let s: String = t.get(&mut lua, "name").unwrap();
    assert_eq!(n, 99);
    assert_eq!(s, "luna");
}

#[test]
fn lua_table_into_global_and_back() {
    let mut lua = Lua::new();
    lua.open_base();
    let t: LuaTable = lua.create_table();
    t.set(&mut lua, "answer", 42_i64).unwrap();
    lua.set_global("c", t).unwrap();
    let r: i64 = lua.eval("return c.answer").unwrap();
    assert_eq!(r, 42);
}

#[test]
fn lua_globals_table_reads_global() {
    let mut lua = Lua::new();
    lua.open_base();
    lua.set_global("magic", 7_i64).unwrap();
    let g: LuaTable = lua.globals();
    let v: i64 = g.get(&mut lua, "magic").unwrap();
    assert_eq!(v, 7);
}

#[test]
fn lua_pin_keeps_value_alive_across_gc() {
    let mut lua = Lua::new();
    lua.open_base();
    let root = lua.pin("immortal");
    // Force a GC cycle; pinned value should still be there.
    lua.vm().collect_garbage();
    let v = root.get(&lua);
    match v {
        Value::Str(s) => assert_eq!(s.as_bytes(), b"immortal"),
        other => panic!("expected Str, got {other:?}"),
    }
}

#[test]
fn lua_unpin_all_resets_pool() {
    let mut lua = Lua::new();
    let _t = lua.create_table();
    let _f = lua.create_function(|| -> i64 { 0 });
    assert_eq!(lua.pinned_count(), 2);
    lua.unpin_all();
    assert_eq!(lua.pinned_count(), 0);
}

#[test]
fn lua_eval_multi_returns() {
    let mut lua = Lua::new();
    lua.open_base();
    let r = lua.eval_multi("return 1, 2, 3").unwrap();
    assert_eq!(r.len(), 3);
    assert!(matches!(r[0], Value::Int(1)));
    assert!(matches!(r[1], Value::Int(2)));
    assert!(matches!(r[2], Value::Int(3)));
}

#[test]
fn lua_function_call_multi() {
    let mut lua = Lua::new();
    let f = lua.create_function(|x: i64| -> (i64, i64) { (x, x * x) });
    let r = f.call_multi(&mut lua, (5_i64,)).unwrap();
    assert_eq!(r.len(), 2);
    assert!(matches!(r[0], Value::Int(5)));
    assert!(matches!(r[1], Value::Int(25)));
}
