//! 5.1/5.2 integer arithmetic once the JITs see it hot. Those dialects have
//! only doubles, so the integers the VM keeps (`#t`, `select('#')`) must
//! round past 2^53 and give -0 like doubles; machine integer code wraps
//! and gives +0.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

fn run(version: LuaVersion, src: &str, jit: bool) -> String {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    match vm.eval(src).expect("eval").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("snippet must return a string, got {other:?}"),
    }
}

fn same(src: &str, expected: &str) {
    let src = format!(
        "local z, one, two = #{{}}, #{{1}}, #{{1, 2}}
         local big = one for i = 1, 32 do big = big * two end
         {src}"
    );
    for v in [LuaVersion::Lua51, LuaVersion::Lua52] {
        assert_eq!(run(v, &src, false), expected, "{v:?} interpreter");
        assert_eq!(run(v, &src, true), expected, "{v:?} JIT");
    }
}

#[test]
fn method_jit_product_of_zero_and_a_negative_is_negative_zero() {
    same(
        "local function mul(a, b) return a * b end
         local r for i = 1, 3000 do r = mul(z, -one) end
         return tostring(1 / r)",
        "-inf",
    );
}

#[test]
fn method_jit_overflowing_product_rounds() {
    same(
        "local function mul(a, b) return a * b end
         local r for i = 1, 3000 do r = mul(big, big) end
         return tostring(r)",
        "1.844674407371e+19",
    );
}

#[test]
fn trace_overflowing_product_rounds() {
    same(
        "local n = #string.rep('x', 3000)
         local acc, i = 0, z
         while i < n do acc = big * big i = i + one end
         return tostring(acc)",
        "1.844674407371e+19",
    );
}

#[test]
fn trace_negated_zero_is_negative_zero() {
    same(
        "local n = #string.rep('x', 3000)
         local acc, i = 0, z
         while i < n do acc = acc + 1 / -z i = i + one end
         return tostring(acc)",
        "-inf",
    );
}
