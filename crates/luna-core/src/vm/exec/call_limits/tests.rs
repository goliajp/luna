//! A Lua stack filled to the limit by what a C function holds on it
//! rather than by a million nested calls: the call that does not fit
//! raises "stack overflow", and a message handler that does not fit the
//! room the overflow leaves it is "error in error handling". Quick enough
//! to run under Miri too.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::exec::stack_size::MAX_STACK;
use crate::vm::{LuaError, Vm};

/// `fill(f)`, as a C function that asks `lua_checkstack` for all but the
/// last thousand slots of the stack's limit, fills them (`lua_settop`) and
/// calls `f` above them.
fn fill(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let top = fs + 1 + nargs;
    let n = MAX_STACK - 1000 - top;
    assert!(vm.checkstack(top, i64::from(n)), "the room is granted");
    vm.native_push_vals(&vec![Value::Nil; n as usize])?;
    vm.call_value(f, &[])?;
    Ok(vm.nat_return(fs, &[]))
}

/// `fill(r)`, `r` recursing past the limit, the error going to the handler
/// `h` (which `calls` counts): the message, and how many times the
/// handler's body ran.
fn overflow(v: LuaVersion, h: &str) -> (String, i64) {
    let src = format!(
        "local function r() return 1 + r() end
         local calls = 0
         local ok, m = xpcall(function() return fill(r) end,
           function(m) calls = calls + 1 {h} end)
         return ok, m, calls"
    );
    let mut vm = Vm::new(v);
    let f = vm.native(fill);
    vm.set_global("fill", f).expect("a global");
    let runs = vm.msgh_runs;
    let r = vm.eval(&src).expect("the script runs");
    // the handler is started once: an overflow in it is "error in error
    // handling" at once, not an error handed to it again
    assert_eq!(vm.msgh_runs - runs, 1, "{v:?}: handler runs");
    assert!(matches!(r[0], Value::Bool(false)), "{v:?}: {:?}", r[0]);
    let Value::Str(m) = r[1] else {
        panic!("{v:?}: {:?}", r[1]);
    };
    let calls = match r[2] {
        Value::Int(i) => i,
        Value::Float(f) => f as i64,
        other => panic!("{v:?}: {other:?}"),
    };
    (String::from_utf8_lossy(m.as_bytes()).into_owned(), calls)
}

#[test]
fn a_full_stack_overflows_and_its_handler_has_only_the_room_left() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let (m, calls) = overflow(v, "return m");
        assert!(m.ends_with(":1: stack overflow"), "{v:?}: {m}");
        assert_eq!(calls, 1, "{v:?}");
        // the handler's own overflow is not handled again
        let (m, calls) = overflow(v, "return r()");
        assert_eq!(m, "error in error handling", "{v:?}");
        assert_eq!(calls, 1, "{v:?}");
    }
}
