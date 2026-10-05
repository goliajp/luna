//! Float `%` and `math.fmod` with NaN operands on Windows. PUC built by
//! MSVC calls the Universal CRT's `fmod`, which returns the second operand
//! whenever it is a NaN and the first otherwise, bits unchanged (a
//! signalling NaN is not quieted). The expected bits below were taken from
//! a C program built with MSVC 19.51 (`cl /O2 /MD`) on windows-latest.
#![cfg(all(windows, target_env = "msvc"))]

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const POS: u64 = 0x7FF8_0000_0000_0000;
const NEG: u64 = 0xFFF8_0000_0000_0000;
const POS1: u64 = 0x7FF8_0000_0000_0001;
const NEG1: u64 = 0xFFF8_0000_0000_0001;
const SNAN: u64 = 0x7FF0_0000_0000_0001;
const NEG_SNAN: u64 = 0xFFF0_0000_0000_0001;
const ONE: u64 = 0x3FF0_0000_0000_0000;

// (a, b, fmod(a, b) as the UCRT gives it)
const CASES: [(u64, u64, u64); 10] = [
    (POS, NEG, NEG),
    (NEG, POS, POS),
    (POS1, NEG1, NEG1),
    (NEG1, POS1, POS1),
    (SNAN, NEG, NEG),
    (NEG, SNAN, SNAN),
    (POS1, NEG_SNAN, NEG_SNAN),
    (NEG_SNAN, ONE, NEG_SNAN),
    (ONE, NEG1, NEG1),
    (NEG1, ONE, NEG1),
];

fn int(v: Value) -> u64 {
    match v {
        Value::Int(i) => i as u64,
        v => panic!("expected an integer, got {v:?}"),
    }
}

#[test]
fn nan_operands_pick_the_nan_the_ucrt_picks() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for (a, b, want) in CASES {
            let src = format!(
                "local function num(bits) return (string.unpack('<d', string.pack('<i8', bits))) end
                 local function bits(x) return (string.unpack('<i8', string.pack('<d', x))) end
                 local a, b = num({}), num({})
                 return bits(a % b), bits(math.fmod(a, b))",
                a as i64, b as i64
            );
            let r = Vm::new(v).eval(&src).expect("run");
            assert_eq!(int(r[0]), want, "{v:?} {a:#x} % {b:#x}");
            assert_eq!(int(r[1]), want, "{v:?} math.fmod({a:#x}, {b:#x})");
        }
    }
}
