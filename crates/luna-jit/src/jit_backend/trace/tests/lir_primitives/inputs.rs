//! The inputs every case runs on.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};

pub(super) const INTS: [i64; 18] = [
    0,
    1,
    -1,
    2,
    7,
    31,
    32,
    63,
    64,
    65,
    255,
    256,
    -1000,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    i64::MAX,
    i64::MIN,
];

pub(super) fn f(x: f64) -> i64 {
    x.to_bits() as i64
}

pub(super) fn floats() -> Vec<i64> {
    [
        0.0,
        -0.0,
        1.5,
        -2.5,
        0.49999999999999994,
        -7.5,
        1e300,
        -1e-310,
        9.223_372_036_854_776e18,
        -9.223_372_036_854_776e18,
        4503599627370497.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ]
    .into_iter()
    .map(f)
    .collect()
}

pub(super) fn pairs(xs: &[i64]) -> Vec<[i64; 4]> {
    xs.iter()
        .flat_map(|&x| xs.iter().map(move |&y| [x, y, 0, 0]))
        .collect()
}

pub(super) fn int_pairs() -> Vec<[i64; 4]> {
    pairs(&INTS)
}

pub(super) fn float_pairs() -> Vec<[i64; 4]> {
    pairs(&floats())
}

pub(super) const INT_CCS: [IntCC; 10] = [
    IntCC::Equal,
    IntCC::NotEqual,
    IntCC::SignedLessThan,
    IntCC::SignedLessThanOrEqual,
    IntCC::SignedGreaterThan,
    IntCC::SignedGreaterThanOrEqual,
    IntCC::UnsignedLessThan,
    IntCC::UnsignedLessThanOrEqual,
    IntCC::UnsignedGreaterThan,
    IntCC::UnsignedGreaterThanOrEqual,
];

pub(super) const FLOAT_CCS: [FloatCC; 6] = [
    FloatCC::Equal,
    FloatCC::NotEqual,
    FloatCC::LessThan,
    FloatCC::LessThanOrEqual,
    FloatCC::GreaterThan,
    FloatCC::GreaterThanOrEqual,
];

pub(super) const IMMS: [i64; 9] = [
    0,
    1,
    -1,
    4095,
    4096,
    -4096,
    0xff00,
    0x0012_3456_789a,
    i64::MIN,
];
