//! Float `%` and `math.fmod` of NaN operands in a compiled trace give the
//! bits the interpreter gives, which follow the C `fmod` PUC is built with
//! on each platform (an x87 `fprem` loop on x86 Linux, the Universal CRT on
//! Windows).

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

const SRC: &str = "
    local function num(bits) return (string.unpack('<d', string.pack('<i8', bits))) end
    local function bits(x) return (string.unpack('<i8', string.pack('<d', x))) end
    local ns = { num(0x7FF8000000000003), num(0xFFF8000000000005), num(0x7FF0000000000001),
                 num(0xFFF8000000000000), num(0x7FF8000000000000), 1.5 }
    local out = {}
    for i = 1, #ns do
      for j = 1, #ns do
        local a, b = ns[i], ns[j]
        local m, f
        for _ = 1, 2000 do m = a % b end
        for _ = 1, 2000 do f = math.fmod(a, b) end
        out[#out + 1] = string.format('%x:%x', bits(m), bits(f))
      end
    end
    return table.concat(out, ' ')";

fn run(version: LuaVersion, jit: bool) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(jit);
    vm.set_trace_jit_enabled(jit);
    let out = match vm.eval(SRC).expect("the snippet runs").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    };
    (out, vm.trace_dispatched_count())
}

#[test]
fn nan_operands_in_a_trace_match_the_interpreter() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let (interp, _) = run(v, false);
        let (jit, dispatched) = run(v, true);
        assert!(dispatched > 0, "{v:?}: no trace was dispatched");
        assert_eq!(jit, interp, "{v:?}: the trace differs from the interpreter");
    }
}
