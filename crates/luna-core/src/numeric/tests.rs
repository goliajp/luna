#[test]
fn strtod_str_reads_what_c_strtod_reads() {
    assert_eq!(strtod_str(b"  10  "), Some(10.0));
    assert_eq!(strtod_str(b"10\0zz"), Some(10.0));
    assert_eq!(strtod_str(b"-inf"), Some(f64::NEG_INFINITY));
    assert_eq!(strtod_str(b"Infinity"), Some(f64::INFINITY));
    assert!(strtod_str(b"nan").is_some_and(f64::is_nan));
    assert!(strtod_str(b"nan(123)").is_some_and(f64::is_nan));
    assert_eq!(strtod_str(b"0x1p4"), Some(16.0));
    assert_eq!(strtod_str(b"0x.8"), Some(0.5));
    assert_eq!(strtod_str(b".5e1"), Some(5.0));
    assert_eq!(strtod_str(b"0x"), None);
    assert_eq!(strtod_str(b"1e"), None);
    assert_eq!(strtod_str(b"infx"), None);
    assert_eq!(strtod_str(b""), None);
}

use super::*;

#[test]
fn str2num_semantics() {
    assert_eq!(str2num(b"  42  ", true, true), Some(Num::Int(42)));
    assert_eq!(str2num(b"-10", true, true), Some(Num::Int(-10)));
    assert_eq!(str2num(b"+0x10", true, true), Some(Num::Int(16)));
    assert_eq!(str2num(b"-0x10", true, true), Some(Num::Int(-16)));
    assert_eq!(str2num(b" 0x1p4 ", true, true), Some(Num::Float(16.0)));
    assert_eq!(str2num(b"3.5", true, true), Some(Num::Float(3.5)));
    assert_eq!(str2num(b"1e3", true, true), Some(Num::Float(1000.0)));
    assert_eq!(str2num(b"", true, true), None);
    assert_eq!(str2num(b" - 1", true, true), None);
    assert_eq!(str2num(b"10a", true, true), None);
    assert_eq!(str2num(b"0x", true, true), None);
    // 5.1 flavor: everything is a float, no hex floats
    assert_eq!(str2num(b"42", false, false), Some(Num::Float(42.0)));
    assert_eq!(str2num(b"0x1p4", false, false), None);
    // minint boundary: "-9223372036854775808" parses as the integer minint
    // (PUC l_str2int's `+ neg`), but the positive magnitude 2^63 overflows
    // to a float, and maxint stays an integer.
    assert_eq!(
        str2num(b"-9223372036854775808", true, true),
        Some(Num::Int(i64::MIN))
    );
    assert_eq!(
        str2num(b"9223372036854775807", true, true),
        Some(Num::Int(i64::MAX))
    );
    assert_eq!(
        str2num(b"9223372036854775808", true, true),
        Some(Num::Float(9223372036854775808.0))
    );
}

#[test]
fn number_printing() {
    assert_eq!(num_to_string(Num::Int(42)), "42");
    assert_eq!(num_to_string(Num::Int(-1)), "-1");
    assert_eq!(num_to_string(Num::Float(2.0)), "2.0");
    assert_eq!(num_to_string(Num::Float(-2.0)), "-2.0");
    assert_eq!(num_to_string(Num::Float(0.5)), "0.5");
    assert_eq!(num_to_string(Num::Float(1e300)), "1e+300");
    assert_eq!(num_to_string(Num::Float(1e-7)), "1e-07");
    assert_eq!(num_to_string(Num::Float(1e15)), "1e+15");
    assert_eq!(num_to_string(Num::Float(100.0)), "100.0");
    assert_eq!(num_to_string(Num::Float(f64::INFINITY)), "inf");
    assert_eq!(num_to_string(Num::Float(f64::NAN)), "nan");
    // PUC 5.5 two-stage rule: %.15g, then %.17g when the round-trip
    // is inexact (lobject.c tostringbuffFloat). Reference spellings
    // taken from the lua5.5 binary (fixture 226 pins the full matrix).
    assert_eq!(num_to_string(Num::Float(0.1)), "0.1");
    assert_eq!(num_to_string(Num::Float(1.0 / 3.0)), "0.33333333333333331");
    assert_eq!(
        num_to_string(Num::Float(std::f64::consts::PI)),
        "3.1415926535897931"
    );
    assert_eq!(num_to_string(Num::Float(1e14)), "100000000000000.0");
    assert_eq!(
        num_to_string(Num::Float(9007199254740992.0)),
        "9007199254740992.0"
    );
    assert_eq!(num_to_string(Num::Float(5e-324)), "4.94065645841247e-324");
}

#[test]
fn hex_float_rounding() {
    // > 53 significant bits forces rounding; Rust's u64→f64 conversion is
    // correctly rounded and serves as the reference
    let Some(Num::Float(f)) = hex_literal(b"1FFFFFFFFFFFFF8.0p0", true, true) else {
        panic!()
    };
    assert_eq!(f, 0x1FFFFFFFFFFFFF8u64 as f64);
    let Some(Num::Float(g)) = hex_literal(b"1.8p1", true, true) else {
        panic!()
    };
    assert_eq!(g, 3.0);
    let Some(Num::Float(h)) = hex_literal(b"0.8", true, true) else {
        panic!()
    };
    assert_eq!(h, 0.5);
}
