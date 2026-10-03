//! Integer `%` with a divisor the hardware division traps on: zero (the
//! interpreter raises) and -1 with `math.mininteger` (the quotient
//! overflows; the result is 0). Run through a Vm with the LLVM backend
//! installed, against the interpreter.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

fn eval(src: &str, jit: bool) -> String {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.install_jit_backend(LlvmBackend, LlvmBackend);
    vm.install_jit_storage(LlvmJitStorage::default());
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    vm.jit.trace_hot_threshold = 1;
    vm.jit.call_hot_threshold = 1;
    let r = vm.eval(src).map_err(|e| vm.error_text(&e)).expect("eval");
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn modulo_by_zero_and_minus_one() {
    let src = r#"
        local function m(a, b) return a % b end
        local function k(a) return a % -1 end
        for i = 1, 100 do m(7, 3) k(i) end
        local out = {}
        for _, p in ipairs({{5, 0}, {math.mininteger, -1}, {7, -1}, {-7, 3}, {7, -3}}) do
          local ok, r = pcall(m, p[1], p[2])
          out[#out + 1] = tostring(ok) .. ":" .. tostring(r)
        end
        out[#out + 1] = tostring(k(math.mininteger))
        local s, ds = 0, {-1, 1, 3, -3, -7}
        for i = 1, 200 do s = s + (i * 7) % ds[i % 5 + 1] end
        out[#out + 1] = tostring(s)
        return table.concat(out, " ")"#;
    let interp = eval(src, false);
    assert!(interp.starts_with("false:"), "{interp}");
    assert_eq!(eval(src, true), interp);
}
