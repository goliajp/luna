//! 5.1 function environments: closures share their creator's `_ENV` cell,
//! and `setfenv` gives its target a new cell, so a change reaches only that
//! function — never a sibling, the creator, or a closure made earlier.

use luna_core::runtime::{ObjTag, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

fn run(src: &str) -> Vec<Value> {
    let mut vm = Vm::new(LuaVersion::Lua51);
    match vm.eval(src) {
        Ok(v) => v,
        Err(e) => panic!("runtime error: {}", vm.error_text(&e)),
    }
}

fn all_true(src: &str) {
    let r = run(src);
    assert!(!r.is_empty());
    for (i, v) in r.iter().enumerate() {
        assert!(
            matches!(v, Value::Bool(true)),
            "result {} of {src:?}: {v:?}",
            i + 1
        );
    }
}

#[test]
fn setfenv_does_not_reach_siblings() {
    all_true(
        "x = 'g' \
         local function mk() return function() return x end end \
         local a, b = mk(), mk() \
         setfenv(a, {x = 'a'}) \
         return a() == 'a', b() == 'g', mk()() == 'g'",
    );
}

#[test]
fn creator_and_children_keep_their_own_env() {
    all_true(
        "x = 'g' \
         local function mk() return function() return x end end \
         local before = mk() \
         setfenv(mk, {x = 'm'}) \
         local after = mk() \
         return before() == 'g', after() == 'm', getfenv(before) == _G, \
                getfenv(after) == getfenv(mk)",
    );
}

#[test]
fn setfenv_on_the_running_function() {
    all_true(
        "x = 'g' \
         local function f() \
           local early = x \
           setfenv(1, {x = 'l'}) \
           return early, x \
         end \
         local e, l = f() \
         local e2, l2 = f() \
         return e == 'g', l == 'l', e2 == 'l', l2 == 'l', x == 'g'",
    );
}

#[test]
fn debug_setfenv_replaces_the_cell_too() {
    all_true(
        "x = 'g' \
         local function mk() return function() return x end end \
         local a, b = mk(), mk() \
         debug.setfenv(a, {x = 'a'}) \
         return a() == 'a', b() == 'g', debug.getfenv(b) == _G",
    );
}

#[test]
fn closures_do_not_allocate_an_env_cell_each() {
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.eval("keep = {} for i = 1, 1000 do keep[i] = function() return i end end collectgarbage()")
        .expect("eval");
    let mut upvalues = 0;
    vm.heap
        .walk_objects(|t| upvalues += usize::from(t == ObjTag::Upvalue));
    // each closure captures its own `i`; none adds an `_ENV` cell of its own
    assert!((1000..1100).contains(&upvalues), "{upvalues} upvalue cells");
}
