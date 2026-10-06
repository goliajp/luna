//! The LLVM method JIT's self-recursive functions: the recursive calls go
//! straight to the compiled body, and still leave it when a callee deep in
//! the recursion cannot go on (a zero divisor), or when the function they
//! go through is no longer the running one.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

fn eval(src: &str) -> Vec<Value> {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend(&mut vm);
    vm.eval(src).unwrap_or_else(|e| panic!("{e}"))
}

fn text(r: &[Value]) -> String {
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => format!("{other:?}"),
    }
}

#[test]
fn a_zero_divisor_deep_in_the_recursion_raises() {
    let r = eval(
        "local function f(n, d) if n < 1 then return 0 end return f(n - 1, d) + n % d end
         local s = 0
         for _ = 1, 300 do s = s + f(40, 7) end
         local ok, e = pcall(f, 40, 0)
         return s .. ' ' .. tostring(ok) .. ' ' .. tostring(e):gsub('^.*: ', '') .. ' ' .. f(40, 7)",
    );
    assert_eq!(text(&r), "36000 false attempt to perform 'n%%0' 120");
}

#[test]
fn recursion_through_a_reassigned_upvalue_follows_it() {
    let r = eval(
        "local f
         f = function(n) if n < 1 then return 0 end return f(n - 1) + 1 end
         local s = 0
         for _ = 1, 300 do s = s + f(30) end
         local g = f
         f = function(n) return 1000 end
         return s .. ' ' .. g(30)",
    );
    assert_eq!(text(&r), "9000 1001");
}
