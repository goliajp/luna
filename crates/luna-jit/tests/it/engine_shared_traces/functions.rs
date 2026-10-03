//! The method JIT's functions shared between Vms.

use super::*;

/// Small integer functions the method JIT compiles.
const FUNCTIONS: &str = r#"
local function add(a, b) return a + b end
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local function floor_half(x) return math.floor(x / 2) end
return function()
    local s = 0
    for i = 1, 300 do s = add(s, i) + floor_half(i) end
    return s + fib(15)
end
"#;

/// Runs `FUNCTIONS` with the method JIT on; returns (results, functions
/// compiled on this thread, functions installed from the engine).
fn run_functions(vm: &mut Vm) -> (Vec<String>, u64, u64) {
    let compiled = luna_jit::jit_backend::chunk_codegen_count();
    let adopted = luna_jit::jit::chunk_adopted_count(vm);
    vm.set_jit_enabled(true);
    let r = run(vm, FUNCTIONS, 3).results;
    (
        r,
        luna_jit::jit_backend::chunk_codegen_count() - compiled,
        luna_jit::jit::chunk_adopted_count(vm) - adopted,
    )
}

#[test]
fn the_method_jit_compiles_nothing_in_a_second_vm() {
    let want = run(&mut interp(LuaVersion::Lua54), FUNCTIONS, 3).results;
    let engine = Engine::new();
    let (r, compiled, _) = run_functions(&mut shared(&engine, LuaVersion::Lua54));
    assert_eq!(r, want);
    assert!(compiled > 0, "the method JIT compiled nothing");
    assert!(engine.function_count() > 0, "nothing was shared");
    let (r, compiled, adopted) = run_functions(&mut shared(&engine, LuaVersion::Lua54));
    assert_eq!(r, want);
    assert_eq!(compiled, 0, "the second Vm compiled functions");
    assert!(adopted > 0, "the second Vm installed no function");
    // another dialect compiles its own
    let (_, compiled, adopted) = run_functions(&mut shared(&engine, LuaVersion::Lua53));
    assert!(compiled > 0);
    assert_eq!(adopted, 0);
}
