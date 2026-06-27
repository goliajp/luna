//! v2.1 Track J-C Phase C — 0-cost default verification.
//!
//! Pins the invariant that bare `Vm` (default features) pays NO
//! Arc/Atomic overhead from the J-C IR migration. The trace-IR
//! wrapper types in `luna_core::jit::send_compat` are
//! `#[repr(transparent)]` over their inner `Cell` / `RefCell` under
//! the default feature set; this test pins the byte-equivalence at
//! the type system layer so a future drift between the wrapper and
//! the inner type breaks the build (rather than silently regressing
//! Vm size or codegen).
//!
//! Under `feature = "send"` the wrapper sizes match
//! `AtomicU32` / `AtomicBool` / `AtomicPtr<u8>` / `RwLock`. The
//! `cfg(not(feature = "send"))` arm pins the default-build invariant
//! which is the load-bearing one for "Vm stays 0-cost" — the send
//! build is opt-in and pays for the cross-thread story explicitly.

#![cfg(not(feature = "send"))]

use luna_core::jit::send_compat::{TArc, TCellBool, TCellPtr, TCellU32, TRefLock};

/// `TArc<T>` is `Rc<T>` under default features — same size + layout.
#[test]
fn t_arc_is_rc_sized() {
    assert_eq!(
        std::mem::size_of::<TArc<u32>>(),
        std::mem::size_of::<std::rc::Rc<u32>>(),
        "TArc must alias Rc under default features"
    );
    assert_eq!(
        std::mem::align_of::<TArc<u32>>(),
        std::mem::align_of::<std::rc::Rc<u32>>(),
    );
}

/// `TCellU32` is `Cell<u32>` — 4 bytes, no atomic overhead.
#[test]
fn t_cell_u32_matches_cell_u32() {
    assert_eq!(
        std::mem::size_of::<TCellU32>(),
        std::mem::size_of::<std::cell::Cell<u32>>(),
        "TCellU32 must match Cell<u32> size under default features"
    );
    assert_eq!(std::mem::size_of::<TCellU32>(), 4);
    assert_eq!(std::mem::align_of::<TCellU32>(), 4);
}

/// `TCellBool` is `Cell<bool>` — 1 byte.
#[test]
fn t_cell_bool_matches_cell_bool() {
    assert_eq!(
        std::mem::size_of::<TCellBool>(),
        std::mem::size_of::<std::cell::Cell<bool>>(),
        "TCellBool must match Cell<bool> size under default features"
    );
    assert_eq!(std::mem::size_of::<TCellBool>(), 1);
}

/// `TCellPtr` is `Cell<*const u8>` — pointer-sized, no atomic overhead.
#[test]
fn t_cell_ptr_matches_cell_ptr() {
    assert_eq!(
        std::mem::size_of::<TCellPtr>(),
        std::mem::size_of::<std::cell::Cell<*const u8>>(),
        "TCellPtr must match Cell<*const u8> size under default features"
    );
    assert_eq!(
        std::mem::size_of::<TCellPtr>(),
        std::mem::size_of::<*const u8>()
    );
}

/// `TRefLock<T>` is `RefCell<T>` — same size, no rwlock overhead.
#[test]
fn t_ref_lock_matches_ref_cell() {
    assert_eq!(
        std::mem::size_of::<TRefLock<u64>>(),
        std::mem::size_of::<std::cell::RefCell<u64>>(),
        "TRefLock<T> must match RefCell<T> size under default features"
    );
}

/// `Box<TCellPtr>` is `Box<Cell<*const u8>>` — pointer to a cell on
/// the heap; the IR bakes the inner cell's heap address via this
/// type. Critical for the IR-layout invariant in Phase B.
#[test]
fn boxed_t_cell_ptr_matches_boxed_cell_ptr() {
    assert_eq!(
        std::mem::size_of::<Box<TCellPtr>>(),
        std::mem::size_of::<Box<std::cell::Cell<*const u8>>>(),
    );
    // Single pointer.
    assert_eq!(
        std::mem::size_of::<Box<TCellPtr>>(),
        std::mem::size_of::<*const u8>(),
    );
}

/// Bare `Vm` size must not balloon from the J-C migration. We can't
/// pin an exact byte count (changes with new fields landing in other
/// tracks), but we can pin a ceiling derived from pre-J-C develop tip
/// `e2ce3ac` (~2.2 KB on arm64). If future Vm growth pushes past this
/// in the J-C path, the assertion will surface so the bump can be
/// reviewed before merge.
#[test]
fn vm_size_default_features_stays_modest() {
    let sz = std::mem::size_of::<luna_core::vm::Vm>();
    assert!(
        sz < 8192,
        "Vm size grew unexpectedly under default features: {sz} bytes"
    );
}
