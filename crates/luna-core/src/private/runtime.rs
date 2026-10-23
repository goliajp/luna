//! The internals of the runtime's types luna's other crates use.

use crate::jit::ExitTag;
use crate::jit::JitVmRebindRestore;
use crate::runtime::Heap;
use crate::runtime::Table;
use crate::runtime::TableError;
use crate::runtime::Value;
use crate::runtime::heap::Gc;
use crate::runtime::mem::RawAllocFn;
use crate::runtime::value::RawVal;
use std::ffi::c_void;

/// The internals of `JitVmGuard` luna's other crates use.
pub trait JitVmGuardInternals {
    /// Construct a guard from a captured restore record. Called only
    /// by luna-jit's `enter_jit`; embedders never call this directly.
    fn from_restore(r: JitVmRebindRestore) -> Self;
}

impl JitVmGuardInternals for crate::jit::JitVmGuard {
    #[inline]
    fn from_restore(r: JitVmRebindRestore) -> Self {
        crate::jit::JitVmGuard::from_restore(r)
    }
}

/// The internals of `CompiledTrace` luna's other crates use.
pub trait CompiledTraceInternals {
    /// The register tags exit `exit_idx` (laid out as `exit_hit_counts`)
    /// leaves, over the whole window from the trace's head frame.
    fn exit_tags_of(&self, exit_idx: usize) -> &[ExitTag];
    /// Where the frame exit `exit_idx` resumes in starts, in registers from
    /// the trace's head frame: the base of the innermost frame an exit
    /// inside an inlined function rebuilds, 0 for an exit in the head frame.
    /// A side trace started at that exit runs on the registers from there.
    fn exit_frame_offset(&self, exit_idx: usize) -> usize;
}

impl CompiledTraceInternals for crate::jit::CompiledTrace {
    #[inline]
    fn exit_tags_of(&self, exit_idx: usize) -> &[ExitTag] {
        crate::jit::CompiledTrace::exit_tags_of(self, exit_idx)
    }
    #[inline]
    fn exit_frame_offset(&self, exit_idx: usize) -> usize {
        crate::jit::CompiledTrace::exit_frame_offset(self, exit_idx)
    }
}

/// The internals of `Heap` luna's other crates use.
pub trait HeapInternals {
    /// A table sized by `NewTable`'s operands, as the interpreter makes it
    /// (see `table::new_table_sizes`); `Ok(None)` for sizes past a table's
    /// limit, an error when memory runs out.
    fn try_new_table_presized(
        &mut self,
        b: u32,
        c: u32,
        k: bool,
    ) -> Result<Option<Gc<Table>>, crate::runtime::table::TableError>;
}

impl HeapInternals for crate::runtime::Heap {
    #[inline]
    fn try_new_table_presized(
        &mut self,
        b: u32,
        c: u32,
        k: bool,
    ) -> Result<Option<Gc<Table>>, crate::runtime::table::TableError> {
        crate::runtime::Heap::try_new_table_presized(self, b, c, k)
    }
}

/// The internals of `Gc` luna's other crates use.
pub trait GcInternals<T> {
    /// Handle to the object at `p`. Panics when `p` is null.
    ///
    /// # Safety
    /// `p` points at a live object of type `T` that a `Heap`(super::Heap)
    /// allocated and still manages, and whoever uses the handle keeps that
    /// object reachable while doing so: `Deref` reads through it.
    unsafe fn from_ptr(p: *mut T) -> Gc<T>;
    /// Exclusive borrow of the referent.
    ///
    /// # Safety
    /// The object is still allocated (reachable from the roots, or not yet
    /// past a collect), no other reference to it is live while the returned
    /// borrow is, and no collect runs while the borrow is held.
    // a `Gc` is a copied handle, as a raw pointer is
    #[allow(clippy::wrong_self_convention)]
    unsafe fn as_mut<'a>(self) -> &'a mut T;
}

impl<T> GcInternals<T> for crate::runtime::heap::Gc<T> {
    #[inline]
    unsafe fn from_ptr(p: *mut T) -> Gc<T> {
        // SAFETY: the caller's contract
        unsafe { crate::runtime::heap::Gc::from_ptr(p) }
    }
    #[inline]
    unsafe fn as_mut<'a>(self) -> &'a mut T {
        // SAFETY: the caller's contract
        unsafe { crate::runtime::heap::Gc::as_mut(self) }
    }
}

/// The internals of `MemCtx` luna's other crates use.
pub trait MemCtxInternals {
    /// Replace the host function (PUC `lua_setallocf`): blocks allocated
    /// so far are resized and freed through the new one.
    ///
    /// # Safety
    /// `f` accepts every live block of the previous function, with `ud`.
    unsafe fn set_raw_alloc(&self, f: RawAllocFn, ud: *mut c_void);
}

impl MemCtxInternals for crate::runtime::mem::MemCtx {
    #[inline]
    unsafe fn set_raw_alloc(&self, f: RawAllocFn, ud: *mut c_void) {
        // SAFETY: the caller's contract
        unsafe { crate::runtime::mem::MemCtx::set_raw_alloc(self, f, ud) }
    }
}

/// The internals of `Table` luna's other crates use.
pub trait TableInternals {
    /// Before a `SetList` stores up to index `last`: grow the array part to
    /// exactly `last` when it is smaller, keeping the hash part's size
    /// (PUC `luaH_resizearray`).
    fn reserve_list(&mut self, heap: &mut Heap, last: u64) -> Result<(), TableError>;
    /// `Self::set` without the read-only test, for compiled code that
    /// has made it already: the trace JIT's store helpers test it together
    /// with the metatable, and the method JIT's only reach tables a call
    /// tested on entry or made itself.
    fn set_unguarded(&mut self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError>;
    /// `Self::set_int` without the read-only test; see
    /// `Self::set_unguarded`.
    fn set_int_unguarded(&mut self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError>;
}

impl TableInternals for crate::runtime::Table {
    #[inline]
    fn reserve_list(&mut self, heap: &mut Heap, last: u64) -> Result<(), TableError> {
        crate::runtime::Table::reserve_list(self, heap, last)
    }
    #[inline]
    fn set_unguarded(&mut self, heap: &mut Heap, key: Value, val: Value) -> Result<(), TableError> {
        crate::runtime::Table::set_unguarded(self, heap, key, val)
    }
    #[inline]
    fn set_int_unguarded(&mut self, heap: &mut Heap, i: i64, val: Value) -> Result<(), TableError> {
        crate::runtime::Table::set_int_unguarded(self, heap, i, val)
    }
}

/// The internals of `Userdata` luna's other crates use.
pub trait UserdataInternals {
    /// A userdata standing for a 5.1 prototype PUC's parser holds on the
    /// stack: its type is "proto".
    fn is_proto(&self) -> bool;
}

impl UserdataInternals for crate::runtime::Userdata {
    #[inline]
    fn is_proto(&self) -> bool {
        crate::runtime::Userdata::is_proto(self)
    }
}

/// The internals of `Value` luna's other crates use.
pub trait ValueInternals {
    /// The array-part encoding of this value. Array tags are the value's
    /// tag with `Bool` split in two (`FALSE`, `TRUE`), so every tag from
    /// `Int` up is one more than the value's; the payload is kept for the
    /// variants that have one and zeroed for `nil` and the booleans.
    fn unpack(self) -> (u8, RawVal);
    /// SAFETY: `(tag, v)` must come from a matching `unpack` of a value that
    /// is still alive.
    unsafe fn pack(tag: u8, v: RawVal) -> Value;
}

impl ValueInternals for crate::runtime::Value {
    #[inline]
    fn unpack(self) -> (u8, RawVal) {
        crate::runtime::Value::unpack(self)
    }
    #[inline]
    unsafe fn pack(tag: u8, v: RawVal) -> Value {
        // SAFETY: the caller's contract
        unsafe { crate::runtime::Value::pack(tag, v) }
    }
}
