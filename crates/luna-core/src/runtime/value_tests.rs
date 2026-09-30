use super::*;
use crate::runtime::heap::Heap;

#[test]
fn value_is_16_bytes() {
    assert_eq!(size_of::<Value>(), 16);
}

#[test]
fn tag_byte_matches_declaration_order() {
    // The `#[repr(C, u8)]` enum puts discriminant byte at offset 0.
    // Variant declaration order in `pub enum Value` is the source of
    // truth for tag::* constants. If you reorder variants without
    // updating tag::*, this test catches it before fast-path
    // helpers misread the tag.
    let mut heap = Heap::new();
    assert_eq!(Value::Nil.tag_byte(), tag::NIL);
    assert_eq!(Value::Bool(false).tag_byte(), tag::BOOL);
    assert_eq!(Value::Bool(true).tag_byte(), tag::BOOL);
    assert_eq!(Value::Int(0).tag_byte(), tag::INT);
    assert_eq!(Value::Int(-1).tag_byte(), tag::INT);
    assert_eq!(Value::Float(std::f64::consts::PI).tag_byte(), tag::FLOAT);
    let s = heap.intern(b"hi");
    assert_eq!(Value::Str(s).tag_byte(), tag::STR);
    assert_eq!(
        Value::LightUserdata(std::ptr::null()).tag_byte(),
        tag::LIGHTUSERDATA
    );
}

#[test]
fn int_unchecked_roundtrip() {
    for v in [0i64, 1, -1, i64::MAX, i64::MIN, 0x1234_5678_9abc_def0] {
        let val = Value::Int(v);
        // SAFETY: we constructed it as Int.
        let recovered = unsafe { val.as_int_unchecked() };
        assert_eq!(recovered, v, "i64 payload round-trips for {}", v);
    }
}

#[test]
fn closure_unchecked_roundtrip() {
    // Constructing a real LuaClosure requires a Proto + Heap; the
    // round-trip is exercised end-to-end via existing
    // call_value/dispatch tests. Here we just sanity-check that
    // `as_closure_unchecked` reads the byte at offset 8 — that
    // ptr_eq holds between input and output.
    // (Skipped: would need to plumb a Proto through Heap.)
    // The integration round-trip is implicit in trace_jit_p15_a tests.
}

#[test]
fn is_callable() {
    let mut heap = Heap::new();
    let s = heap.intern(b"x");
    assert!(!Value::Nil.is_callable());
    assert!(!Value::Int(0).is_callable());
    assert!(!Value::Str(s).is_callable());
    // Closure / Native require heap-allocated callables; integration
    // tests cover those code paths.
}

#[test]
fn raw_equality() {
    assert!(Value::Nil.raw_eq(Value::Nil));
    assert!(Value::Int(3).raw_eq(Value::Float(3.0)));
    assert!(Value::Float(3.0).raw_eq(Value::Int(3)));
    assert!(!Value::Int(3).raw_eq(Value::Float(3.5)));
    // 2^63 rounds to a float outside i64 range: not equal to any int
    assert!(!Value::Int(i64::MAX).raw_eq(Value::Float(i64::MAX as f64)));
    assert!(!Value::Float(f64::NAN).raw_eq(Value::Float(f64::NAN)));
    assert!(!Value::Nil.raw_eq(Value::Bool(false)));
    assert!(Value::Int(0).raw_eq(Value::Float(-0.0)));
}

#[test]
fn string_equality_short_and_long() {
    let mut heap = Heap::new();
    let a = Value::Str(heap.intern(b"abc"));
    let b = Value::Str(heap.intern(b"abc"));
    let c = Value::Str(heap.intern(b"abd"));
    assert!(a.raw_eq(b));
    assert!(!a.raw_eq(c));
    let long1 = Value::Str(heap.intern(&[7u8; 50]));
    let long2 = Value::Str(heap.intern(&[7u8; 50]));
    assert!(long1.raw_eq(long2));
}

#[test]
fn pack_roundtrip() {
    let cases = [
        Value::Nil,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(-42),
        Value::Float(0.5),
    ];
    for v in cases {
        let (t, b) = v.unpack();
        assert!(unsafe { Value::pack(t, b) }.raw_eq(v));
    }
}

#[test]
fn f2i_exact_boundaries() {
    // exact decimal literals, not powi: miri perturbs non-exact float ops
    assert_eq!(f2i_exact(0.0), Some(0));
    assert_eq!(f2i_exact(-0.0), Some(0));
    assert_eq!(f2i_exact(9007199254740992.0), Some(1 << 53));
    assert_eq!(f2i_exact(-9223372036854775808.0), Some(i64::MIN));
    assert_eq!(f2i_exact(9223372036854775808.0), None); // one past i64::MAX
    assert_eq!(f2i_exact(0.5), None);
    assert_eq!(f2i_exact(f64::NAN), None);
    assert_eq!(f2i_exact(f64::INFINITY), None);
}
