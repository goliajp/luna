//! The flag and kind accessors of an object's header.

use super::*;

/// BLACK or a read-only table; see the flag layout on `GcHeader`
pub(crate) const SLOW: u8 = 8;
/// in a string's `sub` byte: luna made the string for itself, PUC has not
/// made it, and the count leaves it out (see `heap_acct.rs`)
pub(crate) const UNCOUNTED: u8 = 0x80;
/// in a string's `sub` byte: a 5.5 external string (its bytes in a block)
pub(crate) const EXTERNAL: u8 = 0x40;
/// in `cls`: PUC does not list the object, whatever its count and finalizer
pub(crate) const UNLISTED: u8 = 1;
/// in `cls`: objects of PUC's stand on it (`PucState::stand_ins`)
pub(crate) const STANDS: u8 = 2;
/// in `cls`: a read-only table; one shift from SLOW
pub(crate) const RO_CLS: u8 = SLOW >> 1;
/// in `cls`: an object of luna's own PUC has none of (`Marker::luna_only`)
pub(crate) const LUNA_ONLY: u8 = 8;
/// in `cls`: a native in one of the heap's blocks of them (`NativeSlab`)
pub(crate) const SLAB: u8 = 0x10;

/// The `aux` bit of a read-only table (`Table::is_readonly`), above its
/// absent-metamethod bits; the JIT's inline stores test it in its byte.
pub(crate) const READONLY_AUX: u32 = 1 << 31;

/// Offsets in `GcHeader`, for the JIT's read-only test.
pub(crate) const AUX_OFFSET: usize = std::mem::offset_of!(GcHeader, aux);
pub(crate) const SUB_OFFSET: usize = std::mem::offset_of!(GcHeader, sub);

/// Discriminator the GC stores in every [`GcHeader`] so a raw header pointer
/// can be cast back to the right object kind during tracing and sweeping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum ObjTag {
    /// [`crate::runtime::string::LuaStr`].
    Str,
    /// [`crate::runtime::table::Table`].
    Table,
    /// [`crate::runtime::function::Proto`].
    Proto,
    /// [`crate::runtime::function::LuaClosure`].
    Closure,
    /// [`crate::runtime::function::Upvalue`].
    Upvalue,
    /// [`crate::runtime::function::NativeClosure`].
    Native,
    /// [`crate::runtime::coroutine::Coro`].
    Coro,
    /// [`crate::runtime::userdata::Userdata`].
    Userdata,
    /// a record of objects PUC made and luna did not (`heap_ghost.rs`)
    Ghost,
}

impl GcHeader {
    /// Byte offset of the flag byte, for compiled code.
    pub(crate) const FLAGS_OFFSET: usize = std::mem::offset_of!(GcHeader, flags);

    /// Whether this is a read-only table (`Table::is_readonly`).
    #[inline(always)]
    pub(crate) fn readonly(&self) -> bool {
        self.tag == ObjTag::Table && self.aux & READONLY_AUX != 0
    }

    /// Whether an in-place store may write into this object as it is:
    /// neither black (a black table needs the write barrier) nor a
    /// read-only table. One bit (SLOW) answers both, so the interpreter's
    /// store fast paths pay one bit test for both; the stores it turns away
    /// go to the slow path, which tells the two apart.
    #[inline(always)]
    pub(crate) fn plain_store(&self) -> bool {
        self.flags & SLOW == 0
    }

    /// Mark or unmark a table read-only, keeping SLOW in step.
    #[inline]
    pub(crate) fn set_readonly(&mut self, on: bool) {
        debug_assert!(self.tag == ObjTag::Table);
        if on {
            self.aux |= READONLY_AUX;
            self.cls |= RO_CLS;
        } else {
            self.aux &= !READONLY_AUX;
            self.cls &= !RO_CLS;
        }
        self.flags = self.with_slow(self.flags);
    }

    /// Flag byte `f`, about to replace this header's, with SLOW set to
    /// agree with its BLACK bit and the read-only mark. Every write of the
    /// colour bits goes through here.
    #[inline(always)]
    pub(crate) fn with_slow(&self, f: u8) -> u8 {
        if f & BLACK != 0 || self.readonly() {
            f | SLOW
        } else {
            f & !SLOW
        }
    }

    /// Whether SLOW agrees with BLACK and the read-only mark.
    #[cfg(any(debug_assertions, feature = "gc-verify"))]
    pub(crate) fn slow_consistent(&self) -> bool {
        self.flags == self.with_slow(self.flags)
    }

    pub(crate) fn new(tag: ObjTag) -> GcHeader {
        GcHeader {
            next: ptr::null_mut(),
            tag,
            flags: if matches!(tag, ObjTag::Str | ObjTag::Ghost) {
                LEAF
            } else {
                0
            },
            sub: 0,
            cls: if tag == ObjTag::Ghost { UNLISTED } else { 0 },
            aux: 0,
        }
    }

    /// A native function's header; one without upvalues has nothing to trace.
    #[inline]
    pub(super) fn native(upvals: &[Value]) -> GcHeader {
        GcHeader {
            flags: if upvals.is_empty() { LEAF } else { 0 },
            ..GcHeader::new(ObjTag::Native)
        }
    }
}
