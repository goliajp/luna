//! 16-byte tagged value (PUC TValue equivalent). Chosen over 8-byte
//! NaN-boxing on bench evidence — see benches/value_repr.rs and the P02 plan:
//! Lua 5.5's native i64 forces NaN-boxed integers into 47-bit smis plus
//! range checks, losing 24% on the arithmetic dispatch path.

use crate::runtime::heap::Gc;
use crate::runtime::string::LuaStr;
use crate::runtime::table::Table;

#[derive(Clone, Copy, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Gc<LuaStr>),
    Table(Gc<Table>),
}

impl Value {
    pub fn type_name(self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
        }
    }

    pub fn is_nil(self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Lua truth: everything except `nil` and `false`.
    pub fn truthy(self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Raw equality (no metamethods): `rawequal` and table-key identity.
    /// Mixed int/float numbers are equal iff the float is exactly integral
    /// and equals the integer (PUC luaV_equalobj F2Ieq rule).
    pub fn raw_eq(self, other: Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => {
                f2i_exact(b) == Some(a)
            }
            (Value::Str(a), Value::Str(b)) => str_eq(a, b),
            (Value::Table(a), Value::Table(b)) => a.ptr_eq(b),
            _ => false,
        }
    }
}

fn str_eq(a: Gc<LuaStr>, b: Gc<LuaStr>) -> bool {
    if a.ptr_eq(b) {
        return true;
    }
    if a.is_short() && b.is_short() {
        return false; // interned: distinct pointers ⇒ distinct contents
    }
    a.as_bytes() == b.as_bytes()
}

/// The float values that convert exactly to i64 (F2Ieq).
pub fn f2i_exact(f: f64) -> Option<i64> {
    if f.trunc() == f && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f) {
        Some(f as i64)
    } else {
        None
    }
}

/// Compact (tag, payload) encoding used by table array parts — the 5.5
/// "compact arrays" layout: 1 tag byte + 8 payload bytes per slot. The
/// payload is a union (PUC `Value` union shape) rather than u64 bits so that
/// pointer provenance survives the round-trip (strict-provenance clean).
pub(crate) mod raw {
    pub const NIL: u8 = 0;
    pub const FALSE: u8 = 1;
    pub const TRUE: u8 = 2;
    pub const INT: u8 = 3;
    pub const FLOAT: u8 = 4;
    pub const STR: u8 = 5;
    pub const TABLE: u8 = 6;

    pub fn is_gc(tag: u8) -> bool {
        tag >= STR
    }
}

#[derive(Clone, Copy)]
pub(crate) union RawVal {
    pub zero: u64,
    pub i: i64,
    pub f: f64,
    pub s: *mut LuaStr,
    pub t: *mut Table,
}

impl RawVal {
    pub(crate) const NIL: RawVal = RawVal { zero: 0 };
}

impl Value {
    pub(crate) fn unpack(self) -> (u8, RawVal) {
        match self {
            Value::Nil => (raw::NIL, RawVal::NIL),
            Value::Bool(false) => (raw::FALSE, RawVal::NIL),
            Value::Bool(true) => (raw::TRUE, RawVal::NIL),
            Value::Int(i) => (raw::INT, RawVal { i }),
            Value::Float(f) => (raw::FLOAT, RawVal { f }),
            Value::Str(s) => (raw::STR, RawVal { s: s.as_ptr() }),
            Value::Table(t) => (raw::TABLE, RawVal { t: t.as_ptr() }),
        }
    }

    /// SAFETY: `(tag, v)` must come from a matching `unpack` of a value that
    /// is still alive.
    pub(crate) unsafe fn pack(tag: u8, v: RawVal) -> Value {
        unsafe {
            match tag {
                raw::NIL => Value::Nil,
                raw::FALSE => Value::Bool(false),
                raw::TRUE => Value::Bool(true),
                raw::INT => Value::Int(v.i),
                raw::FLOAT => Value::Float(v.f),
                raw::STR => Value::Str(Gc::from_ptr(v.s)),
                raw::TABLE => Value::Table(Gc::from_ptr(v.t)),
                _ => unreachable!("bad raw value tag"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::heap::Heap;

    #[test]
    fn value_is_16_bytes() {
        assert_eq!(size_of::<Value>(), 16);
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
}
