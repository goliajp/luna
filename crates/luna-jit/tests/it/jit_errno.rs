//! Compiled code leaves the C library's `errno` (see `luna_core::cerrno`)
//! as the interpreter does: a `math.*` call folded into a trace or a method,
//! a compiled `^` and a compiled float `%`.

use luna_jit::LuaVersion;
use luna_jit::cerrno;

/// `errno` after `src`, starting from 0, and whether compiled code ran: a
/// trace, or a function the method JIT compiled.
fn run(version: LuaVersion, src: &str, jit: bool) -> (i32, bool) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let methods = luna_jit::jit_backend::chunk_codegen_count();
    cerrno::set(0);
    vm.eval(src).expect("eval");
    let compiled =
        vm.trace_dispatched_count() > 0 || luna_jit::jit_backend::chunk_codegen_count() > methods;
    (cerrno::get(), compiled)
}

fn same(version: LuaVersion, src: &str, want: i32) {
    let (interp, _) = run(version, src, false);
    let (jit, compiled) = run(version, src, true);
    assert_eq!(interp, want, "interpreter: {src}");
    assert_eq!(jit, want, "JIT: {src}");
    assert!(compiled, "no compiled code ran: {src}");
}

#[test]
fn folded_math_calls_in_a_trace_set_errno() {
    for (call, arg, want) in [
        ("math.sqrt", "-i", cerrno::EDOM),
        ("math.sqrt", "i", 0),
        ("math.log", "i - i", cerrno::ERANGE),
        ("math.exp", "i + 1000", cerrno::ERANGE),
        ("math.acos", "i + 1", cerrno::EDOM),
    ] {
        let src = format!("local last for i = 1, 20000 do local x = {arg} last = {call}(x) end");
        same(LuaVersion::Lua54, &src, want);
    }
    let fmod = "local last for i = 1, 20000 do local x = i last = math.fmod(x, 0.0) end";
    same(LuaVersion::Lua53, fmod, cerrno::EDOM);
}

#[test]
fn compiled_power_and_modulo_set_errno() {
    let pow = "local a, x = 10.5 for i = 1, 20000 do x = a ^ (i + 400) end";
    same(LuaVersion::Lua53, pow, cerrno::ERANGE);
    // 5.4 squares instead of calling pow
    let square = "local a, x = 1e300 for i = 1, 20000 do x = a ^ 2 end";
    same(LuaVersion::Lua54, square, 0);
    let modulo = "local x for i = 1, 20000 do x = (i + 0.5) % 0.0 end";
    same(LuaVersion::Lua54, modulo, cerrno::EDOM);
}

#[test]
fn folded_math_calls_in_a_method_set_errno() {
    for (call, arg, want) in [
        ("math.log", "0", cerrno::ERANGE),
        ("math.exp", "1000", cerrno::ERANGE),
        ("math.acos", "2", cerrno::EDOM),
        ("math.cos", "1", 0),
    ] {
        let src = format!(
            "local function g(x) local y = {call}(x) return y end
             local r for i = 1, 20000 do r = g({arg}) end"
        );
        same(LuaVersion::Lua54, &src, want);
    }
}
