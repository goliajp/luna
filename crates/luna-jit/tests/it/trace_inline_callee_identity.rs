//! A trace inlines a recursive call as the head function's own body, run
//! with the upvalues of the closure the trace was entered with. That is
//! the callee only while the call target is that very closure: after the
//! recursive local is reassigned, when the target is a different
//! function, or when it is another closure of the same function with
//! other upvalues, the call must go to the real callee.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, method: bool, trace: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(method);
    vm.set_trace_jit_enabled(trace);
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

const DIALECTS: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

#[track_caller]
fn same(src: &str, want: &str) {
    for v in DIALECTS {
        let (interp, _) = run(v, src, false, false);
        assert_eq!(interp, want, "{v:?}: interpreter");
        // the method JIT compiles these functions first when both are on
        let (trace, dispatched) = run(v, src, false, true);
        assert_eq!(
            trace, interp,
            "{v:?}: trace JIT differs from the interpreter"
        );
        assert!(dispatched > 0, "{v:?}: no trace was dispatched");
        let (both, _) = run(v, src, true, true);
        assert_eq!(both, interp, "{v:?}: JIT differs from the interpreter");
    }
}

#[test]
fn recursive_local_reassigned() {
    same(
        "local function f(n) if n < 2 then return n end return f(n - 1) + f(n - 2) end
         local g = f
         local s = 0
         for i = 1, 30 do s = s + g(15) end
         f = function(n) return 1 end
         s = s + g(15)
         return tostring(s)",
        "18302",
    );
}

#[test]
fn call_target_becomes_another_function() {
    same(
        "local function f(n, g) if n == 0 then return 0 end return 1 + g(n - 1, g) end
         local function other(n, g) return 1000 end
         local s = 0
         for i = 1, 300 do s = s + f(5, i <= 200 and f or other) end
         return tostring(s)",
        "101100",
    );
}

/// The inlined frame reads the upvalue `nxt`: it must be the callee's,
/// not the entry closure's.
#[test]
fn call_target_is_another_closure_of_the_function() {
    same(
        "local function make()
           local nxt
           local function f(n)
             if n == 0 then return 0 end
             return 1 + nxt(n - 1)
           end
           return f, function(g) nxt = g end
         end
         local a, seta = make()
         local b, setb = make()
         setb(function(n) return 1000 end)
         seta(a)
         local s = 0
         for i = 1, 300 do s = s + a(5) end
         seta(b)
         for i = 1, 10 do s = s + a(5) end
         return tostring(s)",
        "11520",
    );
}
