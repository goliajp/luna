//! A Lua stack filled to the limit by one call's results rather than by a
//! million nested calls: the call that does not fit raises "stack
//! overflow", and a message handler that does not fit the room the
//! overflow leaves it is "error in error handling". Quick enough to run
//! under Miri too.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::Vm;

/// `string.byte` leaves all but the last thousand slots of the stack's
/// limit as `fill`'s arguments; `fill` then recurses past it, and the
/// error goes to the handler `h` (which `calls` counts): the message, and
/// how many times the handler's body ran.
fn overflow(v: LuaVersion, h: &str) -> (String, i64) {
    let src = format!(
        "local function r() return 1 + r() end
         local function fill(...) return r() end
         local s = string.rep('x', {n})
         local calls = 0
         local ok, m = xpcall(function() return fill(s:byte(1, -1)) end,
           function(m) calls = calls + 1 {h} end)
         return ok, m, calls",
        n = super::stack_size::MAX_STACK - 1000
    );
    let mut vm = Vm::new(v);
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
