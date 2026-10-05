//! A function the LLVM method JIT compiles calls itself on the native
//! stack, past the interpreter's depth limit. Recursion that does not end
//! must still raise Lua's "stack overflow" (it overflowed the process
//! stack and aborted).

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

fn eval(src: &str) -> Result<Vec<Value>, String> {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    luna_jit::install_llvm_backend(&mut vm);
    vm.eval(src).map_err(|e| e.to_string())
}

#[test]
fn recursion_without_a_base_case_raises_stack_overflow() {
    let r = eval(
        "local function f() return f() + 1 end
         local ok, e = pcall(f)
         return tostring(ok) .. ' ' .. tostring(e)",
    )
    .expect("the script catches the error");
    let Some(Value::Str(s)) = r.first() else {
        panic!("{r:?}");
    };
    let s = String::from_utf8_lossy(s.as_bytes()).into_owned();
    assert!(s.starts_with("false ") && s.contains("stack overflow"), "{s}");
}
