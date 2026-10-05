//! A compiled self-recursive call recurses on the native stack, past the
//! interpreter's depth limit, so a function whose every path to a return
//! goes through the recursive call stays in the interpreter: compiled, it
//! overflowed the process stack instead of raising Lua's "stack overflow".

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// Whether the LLVM method JIT compiles the first function `src` defines.
fn compiles(src: &str) -> bool {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm.load(src.as_bytes(), b"=recursion").expect("compile");
    let f = closure.proto.protos[0];
    let r = LlvmBackend.try_compile(&mut LlvmJitStorage::default(), f, false, false);
    matches!(r, CompileResult::Compiled { .. })
}

#[test]
fn recursion_without_a_base_case_is_not_compiled() {
    assert!(!compiles(
        "local function rec(n) local r = rec(n) return r end return rec"
    ));
    assert!(!compiles(
        "local function f(n) return f(n) + 1 end; return f"
    ));
    assert!(!compiles("local function g(n) return g(n) end; return g"));
}

#[test]
fn recursion_with_a_base_case_is_compiled() {
    assert!(compiles(
        "local function rec(n) if n < 1 then return n end local r = rec(n) return r end return rec"
    ));
}
