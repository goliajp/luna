//! The method JIT reads upvalues in two ways — as the target of a
//! recursive call and as a number — and both must still hold when the
//! upvalue is not what the compiled code expects. Each snippet runs with
//! the JIT on and off and must give PUC's result both ways; a chunk
//! whose upvalue does not fit falls back to the interpreter.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

fn run(version: LuaVersion, src: &str, jit: bool) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {e}"),
    }
}

fn same(version: LuaVersion, src: &str, expected: &str) {
    assert_eq!(
        run(version, src, false),
        expected,
        "{version:?} interpreter"
    );
    assert_eq!(run(version, src, true), expected, "{version:?} JIT");
}

/// `a` calls `b` through an upvalue; the compiled body took every such
/// call for a call to itself.
#[test]
fn forward_declared_locals_call_each_other() {
    let src = r#"
        local a, b, c
        a = function(n) if n == 0 then return 0 end; return b(n - 1) + 1 end
        b = function(n) if n == 0 then return 0 end; return c(n - 1) + 2 end
        c = function(n) if n == 0 then return 0 end; return a(n - 1) + 3 end
        return a(10) .. " " .. b(10) .. " " .. c(10)"#;
    for v in ALL {
        same(v, src, "19 20 21");
    }
}

/// The local a recursive function calls itself through can be
/// reassigned; the call then goes to the new function.
#[test]
fn reassigned_recursive_local_calls_the_new_function() {
    let src = r#"
        local function f(n) if n == 0 then return 0 end return f(n - 1) + 1 end
        local g = f
        local first = g(5)
        f = function(n) return 100 end
        return first .. " " .. g(5)"#;
    for v in ALL {
        same(v, src, "5 101");
    }
}
