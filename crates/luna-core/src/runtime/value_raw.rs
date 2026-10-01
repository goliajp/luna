//! A value as the array part stores it (a tag byte and a payload word),
//! and the copy of a value that moves the same two pieces.

use super::*;

impl Value {
    /// The array-part encoding of this value. Array tags are the value's
    /// tag with `Bool` split in two (`FALSE`, `TRUE`), so every tag from
    /// `Int` up is one more than the value's; the payload is kept for the
    /// variants that have one and zeroed for `nil` and the booleans.
    #[doc(hidden)]
    #[inline]
    pub fn unpack(self) -> (u8, RawVal) {
        let t = self.tag_byte();
        if t <= tag::BOOL {
            let truth = matches!(self, Value::Bool(true));
            return (t + truth as u8, RawVal::NIL);
        }
        // SAFETY: from `Int` on every variant has an initialised 8-byte
        // payload at offset 8 (`#[repr(C, u8)]`); copying it as the union
        // keeps a pointer's provenance
        let v = unsafe { *((&self as *const Value as *const u8).add(8) as *const RawVal) };
        (t + 1, v)
    }

    /// SAFETY: `(tag, v)` must come from a matching `unpack` of a value that
    /// is still alive.
    #[doc(hidden)]
    #[inline]
    pub unsafe fn pack(tag: u8, v: RawVal) -> Value {
        let mut out = std::mem::MaybeUninit::<Value>::uninit();
        // SAFETY: the caller's contract is `pack_into`'s
        unsafe {
            Value::pack_into(out.as_mut_ptr(), tag, v);
            out.assume_init()
        }
    }

    /// Copy the value at `src` to `dst` as its tag byte and its payload
    /// word (PUC `setobj`), never as one 16-byte access. A 16-byte load
    /// cannot take its data from the byte-and-word stores that write a
    /// value, and waits for them to reach the cache (8 cycles on a
    /// Cortex-A76); a whole-value copy is compiled to exactly that load.
    ///
    /// # Safety
    /// `src` points at an initialised `Value` and `dst` is writable; they
    /// may be the same.
    #[inline(always)]
    pub(crate) unsafe fn copy_raw(dst: *mut Value, src: *const Value) {
        let (s, d) = (src as *const u8, dst as *mut u8);
        // SAFETY: the caller's contract; the tag is the first byte and the
        // payload (padding for `Nil`, a byte for `Bool`) the second word
        unsafe {
            let t = *s;
            let w = *(s.add(8) as *const std::mem::MaybeUninit<u64>);
            *d = t;
            *(d.add(8) as *mut std::mem::MaybeUninit<u64>) = w;
        }
    }

    /// [`Self::pack`] written straight to `dst`, as a tag byte and a
    /// payload word.
    ///
    /// # Safety
    /// As for `pack`, and `dst` is writable.
    #[doc(hidden)]
    #[inline(always)]
    pub unsafe fn pack_into(dst: *mut Value, tag: u8, v: RawVal) {
        debug_assert!(tag <= raw::LIGHTUSERDATA, "bad raw value tag");
        // SAFETY: the tag is a valid `Value` tag (the array tag, or one
        // less from `Int` up), and the payload is the one `unpack` took
        // from a value of that variant, still alive by the caller's
        // contract
        unsafe {
            if tag <= raw::TRUE {
                dst.write(match tag {
                    raw::NIL => Value::Nil,
                    t => Value::Bool(t == raw::TRUE),
                });
            } else {
                let p = dst as *mut u8;
                *p = tag - 1;
                *(p.add(8) as *mut RawVal) = v;
            }
        }
    }
}
