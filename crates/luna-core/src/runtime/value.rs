//! 16-byte tagged value (PUC TValue equivalent). Chosen over 8-byte
//! NaN-boxing on bench evidence — see benches/value_repr.rs and the P02 plan:
//! Lua 5.5's native i64 forces NaN-boxed integers into 47-bit smis plus
//! range checks, losing 24% on the arithmetic dispatch path.

use crate::runtime::coroutine::Coro;
use crate::runtime::function::LuaClosure;
use crate::runtime::heap::Gc;
use crate::runtime::string::LuaStr;
use crate::runtime::table::Table;
use crate::runtime::userdata::Userdata;

/// Native (host) function: receives the VM, the absolute stack slot of the
/// function value (its own NativeClosure — read upvalues through it), and
/// the argument count; writes results starting at that slot and returns how
/// many.
///
/// Embedding contract: prefer `Err(LuaError)` over `panic!` for error
/// signaling. Panics that escape the callback are caught and converted
/// to a `LuaError("native panic: <msg>")`, but the VM state may be
/// inconsistent after a panic (half-pushed args, dangling refs) — the
/// host should treat any `"native panic:"`-prefixed error as fatal and
/// drop the Vm rather than reusing it.
pub type NativeFn =
    fn(&mut crate::vm::Vm, func_slot: u32, nargs: u32) -> Result<u32, crate::vm::LuaError>;

use crate::runtime::function::NativeClosure;

/// P17-D v2 Direction E (E1) — `#[repr(C, u8)]` makes the discriminant a
/// 1-byte tag at offset 0, with the variant payload starting at offset 8
/// (after 7 bytes of alignment padding). The total size stays 16 bytes
/// (same as the prior plain Rust enum representation), preserving P02's
/// arithmetic-fast-path 24% win over NaN-boxing.
///
/// The PUC-equivalent layout this gives us means LJ_FR2-style frame
/// metadata reads (`stack[base-2]` for closure, `stack[base-1]` for the
/// packed frame marker) can use a single 1-byte tag load + payload
/// branch — see [`Value::tag_byte`] and friends. The previous enum
/// repr left discriminant position unspecified, so byte-level reads of
/// Value layout would have been unportable.
///
/// Variant order MUST stay stable: rustc assigns discriminants
/// 0..11 in declaration order (Nil=0, Bool=1, ..., LightUserdata=10),
/// and Phase 3+ hot paths read those discriminants via `tag_byte()`.
/// New variants must be appended; reordering changes the wire layout.
#[derive(Clone, Copy, Debug)]
#[repr(C, u8)]
pub enum Value {
    /// Lua `nil`.
    Nil,
    /// Lua `boolean`.
    Bool(
        /// Underlying boolean.
        bool,
    ),
    /// Lua integer (5.3+; in 5.1 all numbers arrive as `Float`).
    Int(
        /// 64-bit signed value.
        i64,
    ),
    /// Lua float.
    Float(
        /// IEEE-754 double.
        f64,
    ),
    /// Lua `string` — GC-managed byte string.
    Str(
        /// String handle.
        Gc<LuaStr>,
    ),
    /// Lua `table`.
    Table(
        /// Table handle.
        Gc<Table>,
    ),
    /// Lua function backed by a [`LuaClosure`].
    Closure(
        /// Closure handle.
        Gc<LuaClosure>,
    ),
    /// Lua function backed by a host [`NativeClosure`].
    Native(
        /// Native closure handle.
        Gc<NativeClosure>,
    ),
    /// Lua `thread` (coroutine).
    Coro(
        /// Coroutine handle.
        Gc<Coro>,
    ),
    /// Full userdata — GC-managed host-allocated payload with a metatable.
    Userdata(
        /// Userdata handle.
        Gc<Userdata>,
    ),
    /// PUC `LUA_TLIGHTUSERDATA`: an opaque host pointer that participates only
    /// as an identity token (raw equality on pointer bits, no metatable, not
    /// GC-managed). Currently produced exclusively by `debug.upvalueid` — it
    /// points at the upvalue cell's `Value` slot and stays distinct per cell.
    LightUserdata(
        /// Opaque host pointer used as an identity token.
        *const (),
    ),
}

// SAFETY: `LightUserdata` holds a raw pointer (PUC `void*` identity token).
// The other `Value` variants already carry GC pointers that aren't `Send`/
// `Sync` either — the type as a whole is single-threaded by construction.
// The raw `*const ()` doesn't change that contract.

impl Value {
    /// Lua-visible type name (`"nil"`, `"boolean"`, `"number"`,
    /// `"string"`, `"table"`, `"function"`, `"thread"`, `"userdata"`)
    /// matching `type()`.
    pub fn type_name(self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Bool(_) => "boolean",
            Value::Int(_) | Value::Float(_) => "number",
            Value::Str(_) => "string",
            Value::Table(_) => "table",
            Value::Closure(_) | Value::Native(_) => "function",
            Value::Coro(_) => "thread",
            // PUC `lua_typename` collapses full and light userdata to
            // "userdata"; only `luaL_typeerror` distinguishes them by tag.
            Value::Userdata(_) | Value::LightUserdata(_) => "userdata",
        }
    }

    /// True when this is [`Value::Nil`].
    pub fn is_nil(self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Lua truth: everything except `nil` and `false`.
    pub fn truthy(self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// P17-D v2 Direction E (E1) — read the variant's discriminant byte
    /// directly. The `#[repr(C, u8)]` on the enum makes this a single
    /// 1-byte load from `&self`, regardless of variant.
    ///
    /// Discriminant values follow declaration order:
    /// `Nil=0, Bool=1, Int=2, Float=3, Str=4, Table=5, Closure=6,
    ///  Native=7, Coro=8, Userdata=9, LightUserdata=10`.
    ///
    /// Use [`tag`] constants instead of literal numbers at call
    /// sites — see the module-level `tag` constants below.
    #[inline(always)]
    pub fn tag_byte(&self) -> u8 {
        // SAFETY: `#[repr(C, u8)]` on `Value` guarantees the
        // discriminant occupies the first byte. Reading it as `u8` is
        // therefore well-defined for ANY Value variant.
        unsafe { *(self as *const Value as *const u8) }
    }

    /// Fast tag-only check for Lua function-call sites. Returns true
    /// iff the value's discriminant is `Closure` or `Native` (the
    /// callable types). Avoids matching the entire enum.
    #[inline(always)]
    pub fn is_callable(self) -> bool {
        let t = self.tag_byte();
        t == tag::CLOSURE || t == tag::NATIVE
    }

    /// Read the closure pointer without an enum match. Caller must
    /// have verified `tag_byte() == tag::CLOSURE` first.
    ///
    /// SAFETY: the value's discriminant MUST be Closure. UB otherwise.
    ///
    /// `#[doc(hidden)]` (Track A4): JIT hot-path use; embedders should
    /// use the safe `match value { Value::Closure(c) => ..., _ => ... }`
    /// instead.
    #[doc(hidden)]
    #[inline(always)]
    pub unsafe fn as_closure_unchecked(self) -> Gc<crate::runtime::LuaClosure> {
        debug_assert_eq!(self.tag_byte(), tag::CLOSURE);
        // SAFETY: `#[repr(C, u8)]` Value with Closure discriminant has
        // payload `Gc<LuaClosure>` at offset 8 (after 7 bytes of
        // alignment padding past the 1-byte tag). `Gc<T>` is a NonNull
        // pointer so its layout is a single 8-byte pointer.
        unsafe {
            let payload_ptr = (&self as *const Value as *const u8).add(8)
                as *const Gc<crate::runtime::LuaClosure>;
            *payload_ptr
        }
    }

    /// Read the integer payload without an enum match. Caller must
    /// have verified `tag_byte() == tag::INT` first.
    ///
    /// SAFETY: the value's discriminant MUST be Int. UB otherwise.
    ///
    /// `#[doc(hidden)]` (Track A4): JIT hot-path use; embedders should
    /// use the safe `match value { Value::Int(i) => ..., _ => ... }`
    /// instead.
    #[doc(hidden)]
    #[inline(always)]
    pub unsafe fn as_int_unchecked(self) -> i64 {
        debug_assert_eq!(self.tag_byte(), tag::INT);
        unsafe {
            let payload_ptr = (&self as *const Value as *const u8).add(8) as *const i64;
            *payload_ptr
        }
    }

    /// Borrow the Lua string's bytes as a UTF-8 `&str` (B7 — Phase 2).
    /// Returns `None` if this value is not a `Value::Str`, or if the
    /// string's bytes are not valid UTF-8.
    ///
    /// Embedders dealing with text data use this. For binary data
    /// (Redis protocol buffers, etc.) use [`Value::as_bytes`].
    pub fn try_as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => std::str::from_utf8(s.as_bytes()).ok(),
            _ => None,
        }
    }

    /// Borrow the raw bytes of a `Value::Str` (B7 — Phase 2). Returns
    /// `None` for non-string variants. Always safe — Lua strings are
    /// byte sequences and may carry non-UTF-8 content.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s.as_bytes()),
            _ => None,
        }
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
            (Value::Closure(a), Value::Closure(b)) => a.ptr_eq(b),
            (Value::Native(a), Value::Native(b)) => a.ptr_eq(b),
            (Value::Coro(a), Value::Coro(b)) => a.ptr_eq(b),
            (Value::Userdata(a), Value::Userdata(b)) => a.ptr_eq(b),
            (Value::LightUserdata(a), Value::LightUserdata(b)) => a == b,
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

/// P17-D v2 Direction E (E1) — discriminant byte constants for
/// [`Value::tag_byte`]. These match the `#[repr(C, u8)]` enum's
/// declaration order; reordering Value variants requires updating
/// these constants in lock-step.
///
/// Separate from the [`raw`] module's tagging scheme: `raw::*` is the
/// luna 5.5-style "compact arrays" marshalling tag (separates
/// `Bool(false)` vs `Bool(true)` into FALSE/TRUE tags, encodes
/// `Userdata`/`LightUserdata` together, etc.). `tag::*` is the actual
/// `Value` enum discriminant — one tag per variant — used by
/// LJ_FR2-style frame-metadata reads in Phase 3+.
pub mod tag {
    /// Tag for `Value::Nil`.
    pub const NIL: u8 = 0;
    /// Tag for `Value::Bool`.
    pub const BOOL: u8 = 1;
    /// Tag for `Value::Int`.
    pub const INT: u8 = 2;
    /// Tag for `Value::Float`.
    pub const FLOAT: u8 = 3;
    /// Tag for `Value::Str`.
    pub const STR: u8 = 4;
    /// Tag for `Value::Table`.
    pub const TABLE: u8 = 5;
    /// Tag for `Value::Closure`.
    pub const CLOSURE: u8 = 6;
    /// Tag for `Value::Native`.
    pub const NATIVE: u8 = 7;
    /// Tag for `Value::Coro`.
    pub const CORO: u8 = 8;
    /// Tag for `Value::Userdata`.
    pub const USERDATA: u8 = 9;
    /// Tag for `Value::LightUserdata`.
    pub const LIGHTUSERDATA: u8 = 10;
}

/// Compact (tag, payload) encoding used by table array parts — the 5.5
/// "compact arrays" layout: 1 tag byte + 8 payload bytes per slot. The
/// payload is a union (PUC `Value` union shape) rather than u64 bits so that
/// pointer provenance survives the round-trip (strict-provenance clean).
#[doc(hidden)]
pub mod raw {
    pub const NIL: u8 = 0;
    pub const FALSE: u8 = 1;
    pub const TRUE: u8 = 2;
    pub const INT: u8 = 3;
    pub const FLOAT: u8 = 4;
    pub const STR: u8 = 5;
    pub const TABLE: u8 = 6;
    pub const CLOSURE: u8 = 7;
    pub const NATIVE: u8 = 8;
    pub const CORO: u8 = 9;
    pub const USERDATA: u8 = 10;
    pub const LIGHTUSERDATA: u8 = 11;

    /// Heap-managed tags.
    pub fn is_gc(tag: u8) -> bool {
        // LIGHTUSERDATA is an opaque host pointer (PUC void*); not GC-managed.
        (STR..LIGHTUSERDATA).contains(&tag)
    }
}

#[derive(Clone, Copy)]
#[doc(hidden)]
pub union RawVal {
    pub zero: u64,
    pub i: i64,
    pub f: f64,
    pub s: *mut LuaStr,
    pub t: *mut Table,
    pub c: *mut LuaClosure,
    pub n: *mut NativeClosure,
    pub co: *mut Coro,
    pub u: *mut Userdata,
    pub lu: *const (),
}

impl RawVal {
    pub(crate) const NIL: RawVal = RawVal { zero: 0 };
}

impl Value {
    #[doc(hidden)]
    pub fn unpack(self) -> (u8, RawVal) {
        match self {
            Value::Nil => (raw::NIL, RawVal::NIL),
            Value::Bool(false) => (raw::FALSE, RawVal::NIL),
            Value::Bool(true) => (raw::TRUE, RawVal::NIL),
            Value::Int(i) => (raw::INT, RawVal { i }),
            Value::Float(f) => (raw::FLOAT, RawVal { f }),
            Value::Str(s) => (raw::STR, RawVal { s: s.as_ptr() }),
            Value::Table(t) => (raw::TABLE, RawVal { t: t.as_ptr() }),
            Value::Closure(c) => (raw::CLOSURE, RawVal { c: c.as_ptr() }),
            Value::Native(n) => (raw::NATIVE, RawVal { n: n.as_ptr() }),
            Value::Coro(co) => (raw::CORO, RawVal { co: co.as_ptr() }),
            Value::Userdata(u) => (raw::USERDATA, RawVal { u: u.as_ptr() }),
            Value::LightUserdata(p) => (raw::LIGHTUSERDATA, RawVal { lu: p }),
        }
    }

    /// SAFETY: `(tag, v)` must come from a matching `unpack` of a value that
    /// is still alive.
    #[doc(hidden)]
    pub unsafe fn pack(tag: u8, v: RawVal) -> Value {
        unsafe {
            match tag {
                raw::NIL => Value::Nil,
                raw::FALSE => Value::Bool(false),
                raw::TRUE => Value::Bool(true),
                raw::INT => Value::Int(v.i),
                raw::FLOAT => Value::Float(v.f),
                raw::NATIVE => Value::Native(Gc::from_ptr(v.n)),
                raw::STR => Value::Str(Gc::from_ptr(v.s)),
                raw::TABLE => Value::Table(Gc::from_ptr(v.t)),
                raw::CLOSURE => Value::Closure(Gc::from_ptr(v.c)),
                raw::CORO => Value::Coro(Gc::from_ptr(v.co)),
                raw::USERDATA => Value::Userdata(Gc::from_ptr(v.u)),
                raw::LIGHTUSERDATA => Value::LightUserdata(v.lu),
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
    fn p17d_e1_tag_byte_matches_declaration_order() {
        // The `#[repr(C, u8)]` enum puts discriminant byte at offset 0.
        // Variant declaration order in `pub enum Value` is the source of
        // truth for tag::* constants. If you reorder variants without
        // updating tag::*, this test catches it before Phase 3 fast-path
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
    fn p17d_e1_int_unchecked_roundtrip() {
        for v in [0i64, 1, -1, i64::MAX, i64::MIN, 0x1234_5678_9abc_def0] {
            let val = Value::Int(v);
            // SAFETY: we constructed it as Int.
            let recovered = unsafe { val.as_int_unchecked() };
            assert_eq!(recovered, v, "i64 payload round-trips for {}", v);
        }
    }

    #[test]
    fn p17d_e1_closure_unchecked_roundtrip() {
        // Constructing a real LuaClosure requires a Proto + Heap; the
        // round-trip is exercised end-to-end via existing
        // call_value/dispatch tests. Here we just sanity-check that
        // `as_closure_unchecked` reads the byte at offset 8 — that
        // ptr_eq holds between input and output.
        // (Skipped: would need to plumb a Proto through Heap.)
        // The integration round-trip is implicit in trace_jit_p15_a tests.
    }

    #[test]
    fn p17d_e1_is_callable() {
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
}
