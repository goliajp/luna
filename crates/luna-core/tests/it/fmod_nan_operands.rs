//! Float `%` and `math.fmod` with two NaN operands on x86 Linux. PUC built
//! by gcc there computes `fmod` with the x87 `fprem` instruction, which
//! returns the NaN with the larger significand (quieted), and the positive
//! one when the two differ only in sign. The expected bits below were taken
//! from gcc-compiled C on x86_64 glibc.
#![cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "x86")))]

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const NEG: u64 = 0xFFF8_0000_0000_0000;
const POS: u64 = 0x7FF8_0000_0000_0000;

// (a, b, a % b as fprem gives it)
const CASES: [(u64, u64, u64); 9] = [
    (NEG, POS, POS),
    (POS, NEG, POS),
    (NEG, NEG, NEG),
    (POS, POS, POS),
    (
        0x7FF8_0000_0000_0003,
        0xFFF8_0000_0000_0005,
        0xFFF8_0000_0000_0005,
    ),
    (
        0xFFF8_0000_0000_0005,
        0x7FF8_0000_0000_0003,
        0xFFF8_0000_0000_0005,
    ),
    // a signalling NaN is quieted before the comparison
    (
        0x7FF0_0000_0000_0001,
        0xFFF8_0000_0000_0000,
        0x7FF8_0000_0000_0001,
    ),
    (
        0xFFF0_0000_0000_0009,
        0x7FF8_0000_0000_0003,
        0xFFF8_0000_0000_0009,
    ),
    (
        0x7FF4_0000_0000_0000,
        0xFFFC_0000_0000_0000,
        0x7FFC_0000_0000_0000,
    ),
];

fn int(v: Value) -> u64 {
    match v {
        Value::Int(i) => i as u64,
        v => panic!("expected an integer, got {v:?}"),
    }
}

#[test]
fn two_nan_operands_pick_the_nan_fprem_picks() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for (a, b, want) in CASES {
            let src = format!(
                "local function nan(bits) return (string.unpack('<d', string.pack('<i8', bits))) end
                 local function bits(x) return (string.unpack('<i8', string.pack('<d', x))) end
                 local a, b = nan({}), nan({})
                 return bits(a % b), bits(math.fmod(a, b))",
                a as i64, b as i64
            );
            let r = Vm::new(v).eval(&src).expect("run");
            assert_eq!(int(r[0]), want, "{v:?} {a:#x} % {b:#x}");
            assert_eq!(int(r[1]), want, "{v:?} math.fmod({a:#x}, {b:#x})");
        }
    }
}
