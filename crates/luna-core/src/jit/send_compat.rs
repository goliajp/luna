//! v2.1 Track J-C — cfg-gated Send-friendly aliases & wrappers for
//! trace IR interior-mutability types.
//!
//! Two cfg modes:
//! - `#[cfg(not(feature = "send"))]` — the default. All aliases
//!   resolve to `Rc` / `Cell` / `RefCell`; identical layout, identical
//!   behavior, identical perf as pre-J-C. **Bare `Vm` stays 0-cost**.
//! - `#[cfg(feature = "send")]` — Send build. Aliases switch to
//!   `Arc` / `AtomicU32` / `AtomicBool` / `AtomicPtr<u8>` / `RwLock`
//!   so `CompiledTrace` + `Proto.traces` become structurally `Send +
//!   Sync` (combined with the J-A/J-B/J-D/J-E sleeves). No `unsafe
//!   impl Send` lifted in J-C — the lifts happen because the inner
//!   types are already Send.
//!
//! Wrapper newtypes (`TCellU32`, `TCellBool`, `TCellPtr`, `TRefLock`)
//! expose Cell/RefCell-shaped methods (`.get()`/`.set()`/`.borrow()`/
//! `.borrow_mut()`) so call sites stay path-identical to the pre-J-C
//! `std::cell::*` shape — the cfg switch happens inside the wrapper.
//!
//! `TArc<T>` is a pure type alias to `Rc<T>` or `Arc<T>`. Both
//! stdlib types share the same inherent-method names
//! (`::new` / `::from` / `::clone` / `::as_ptr` / `::strong_count`)
//! so the alias works for all call shapes that previously used
//! `std::rc::Rc`.
//!
//! Performance note (default build): all wrappers are
//! `#[repr(transparent)]` over their inner `Cell` / `RefCell`. The
//! generated code is identical to direct Cell/RefCell access (the
//! wrapper methods inline trivially). Size assertions in
//! `tests/j_c_zero_cost_default.rs` pin this.
//!
//! See `.dev/rfcs/v2.1-track-j-c-verdict.md` for the full migration
//! list + cfg-gating pattern shape.

// ============================================================
// TArc<T> — `Rc<T>` (default) or `Arc<T>` (send).
// ============================================================

/// J-C cfg-gated reference count. `Rc<T>` under the default feature
/// set, `Arc<T>` under `feature = "send"`. Construction and method
/// shapes match between the two (`new`, `from`, `clone`, `as_ptr`,
/// `strong_count`), so most call sites swap `Rc` → `TArc` without
/// further changes.
#[cfg(not(feature = "send"))]
pub type TArc<T> = std::rc::Rc<T>;
/// J-C cfg-gated reference count (send build). See `feature = "send"`-off
/// alias above.
#[cfg(feature = "send")]
pub type TArc<T> = std::sync::Arc<T>;

// ============================================================
// TCellU32 — Cell<u32> (default) or AtomicU32 (send).
// ============================================================

/// J-C cfg-gated `u32` cell. Same API surface as `std::cell::Cell<u32>`
/// (`new`, `get`, `set`); the send build swaps in `AtomicU32` with
/// `Relaxed` ordering — the SendVm RwLock supplies the cross-thread
/// happens-before, so per-op atomic ordering can be relaxed.
#[repr(transparent)]
#[derive(Debug)]
pub struct TCellU32 {
    #[cfg(not(feature = "send"))]
    inner: std::cell::Cell<u32>,
    #[cfg(feature = "send")]
    inner: std::sync::atomic::AtomicU32,
}

impl TCellU32 {
    /// Construct a new cell holding `v`.
    #[inline]
    pub const fn new(v: u32) -> Self {
        #[cfg(not(feature = "send"))]
        {
            Self {
                inner: std::cell::Cell::new(v),
            }
        }
        #[cfg(feature = "send")]
        {
            Self {
                inner: std::sync::atomic::AtomicU32::new(v),
            }
        }
    }

    /// Read the current value.
    #[inline]
    pub fn get(&self) -> u32 {
        #[cfg(not(feature = "send"))]
        {
            self.inner.get()
        }
        #[cfg(feature = "send")]
        {
            self.inner.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    /// Store `v` into the cell.
    #[inline]
    pub fn set(&self, v: u32) {
        #[cfg(not(feature = "send"))]
        {
            self.inner.set(v);
        }
        #[cfg(feature = "send")]
        {
            self.inner.store(v, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

impl Clone for TCellU32 {
    fn clone(&self) -> Self {
        Self::new(self.get())
    }
}

impl Default for TCellU32 {
    fn default() -> Self {
        Self::new(0)
    }
}

// ============================================================
// TCellBool — Cell<bool> (default) or AtomicBool (send).
// ============================================================

/// J-C cfg-gated `bool` cell. Same API as `std::cell::Cell<bool>`
/// (`new`, `get`, `set`).
#[repr(transparent)]
#[derive(Debug)]
pub struct TCellBool {
    #[cfg(not(feature = "send"))]
    inner: std::cell::Cell<bool>,
    #[cfg(feature = "send")]
    inner: std::sync::atomic::AtomicBool,
}

impl TCellBool {
    /// Construct a new cell holding `v`.
    #[inline]
    pub const fn new(v: bool) -> Self {
        #[cfg(not(feature = "send"))]
        {
            Self {
                inner: std::cell::Cell::new(v),
            }
        }
        #[cfg(feature = "send")]
        {
            Self {
                inner: std::sync::atomic::AtomicBool::new(v),
            }
        }
    }

    /// Read the current value.
    #[inline]
    pub fn get(&self) -> bool {
        #[cfg(not(feature = "send"))]
        {
            self.inner.get()
        }
        #[cfg(feature = "send")]
        {
            self.inner.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    /// Store `v` into the cell.
    #[inline]
    pub fn set(&self, v: bool) {
        #[cfg(not(feature = "send"))]
        {
            self.inner.set(v);
        }
        #[cfg(feature = "send")]
        {
            self.inner.store(v, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

// ============================================================
// TCellPtr — Cell<*const u8> (default) or AtomicPtr<u8> (send).
// ============================================================

/// J-C cfg-gated raw-pointer cell. Same API as `std::cell::Cell<*const u8>`
/// (`new`, `get`, `set`).
///
/// **IR layout invariant** (preserved): both `Cell<*const u8>` and
/// `AtomicPtr<u8>` are 8-byte-sized, pointer-aligned, and store the
/// raw pointer bits at offset 0. The Cranelift IR emits
/// `iconst(I64, cell_addr) + load.i64` to read these cells; under
/// `feature = "send"` the same load reads the AtomicPtr's bits with
/// equivalent semantics — `AtomicPtr::load(Relaxed)` lowers to a plain
/// pointer-sized load on the targets luna supports (arm64, x86_64),
/// matching the pre-J-C `Cell::get` codegen byte-for-byte.
#[repr(transparent)]
pub struct TCellPtr {
    #[cfg(not(feature = "send"))]
    inner: std::cell::Cell<*const u8>,
    #[cfg(feature = "send")]
    inner: std::sync::atomic::AtomicPtr<u8>,
}

impl std::fmt::Debug for TCellPtr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TCellPtr")
            .field("ptr", &self.get())
            .finish()
    }
}

impl TCellPtr {
    /// Construct a new cell holding the null pointer.
    #[inline]
    pub const fn null() -> Self {
        #[cfg(not(feature = "send"))]
        {
            Self {
                inner: std::cell::Cell::new(std::ptr::null()),
            }
        }
        #[cfg(feature = "send")]
        {
            Self {
                inner: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            }
        }
    }

    /// Construct a new cell holding `p`.
    #[inline]
    pub fn new(p: *const u8) -> Self {
        #[cfg(not(feature = "send"))]
        {
            Self {
                inner: std::cell::Cell::new(p),
            }
        }
        #[cfg(feature = "send")]
        {
            Self {
                inner: std::sync::atomic::AtomicPtr::new(p as *mut u8),
            }
        }
    }

    /// Read the current pointer bits.
    #[inline]
    pub fn get(&self) -> *const u8 {
        #[cfg(not(feature = "send"))]
        {
            self.inner.get()
        }
        #[cfg(feature = "send")]
        {
            self.inner.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    /// Store `p` into the cell.
    #[inline]
    pub fn set(&self, p: *const u8) {
        #[cfg(not(feature = "send"))]
        {
            self.inner.set(p);
        }
        #[cfg(feature = "send")]
        {
            self.inner
                .store(p as *mut u8, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Address of the cell itself — the value Cranelift IR bakes as
    /// `iconst(I64, _)` to issue direct loads at runtime.
    #[inline]
    pub fn cell_addr(&self) -> *const () {
        self as *const _ as *const ()
    }
}

/// Clone mirrors `Cell<*const u8>: Clone` (Cell<T> is Clone whenever
/// T: Copy). Produces a new cell at a different heap location with
/// the same pointer bits. Callers that rely on heap-address stability
/// (Cranelift IR loads of side-trace ptr cells) must NOT clone the
/// containing `Box<TCellPtr>` once the IR has baked the original's
/// address — same invariant as pre-J-C `Box<Cell<*const u8>>`.
impl Clone for TCellPtr {
    fn clone(&self) -> Self {
        Self::new(self.get())
    }
}

// ============================================================
// TRefLock<T> — RefCell<T> (default) or RwLock<T> (send).
// ============================================================

/// J-C cfg-gated interior-mutable lock. Exposes `RefCell`-shaped
/// `.borrow()` / `.borrow_mut()` whose returned guards `Deref<Target =
/// T>`. Under the default feature set this is a thin newtype around
/// `RefCell<T>` (zero overhead vs. the pre-J-C `RefCell` field);
/// under `feature = "send"` it wraps `RwLock<T>` and the guards
/// become `RwLockReadGuard` / `RwLockWriteGuard`.
///
/// Lock failure handling: the send build `unwrap()`s the lock result.
/// The SendVm's outer `RwLock<()>` serializes mutator access so a
/// poisoned lock is a real bug (a panic in a guard's user) and the
/// propagating panic is the same UX as `RefCell::borrow_mut` on a
/// re-entrant borrow.
#[repr(transparent)]
#[derive(Debug)]
pub struct TRefLock<T: ?Sized> {
    #[cfg(not(feature = "send"))]
    inner: std::cell::RefCell<T>,
    #[cfg(feature = "send")]
    inner: std::sync::RwLock<T>,
}

impl<T> TRefLock<T> {
    /// Construct a new lock around `v`.
    #[inline]
    pub const fn new(v: T) -> Self {
        #[cfg(not(feature = "send"))]
        {
            Self {
                inner: std::cell::RefCell::new(v),
            }
        }
        #[cfg(feature = "send")]
        {
            Self {
                inner: std::sync::RwLock::new(v),
            }
        }
    }

    /// Borrow the lock immutably. The returned guard derefs to `&T`;
    /// callers use it identically to `RefCell::borrow`.
    #[cfg(not(feature = "send"))]
    #[inline]
    pub fn borrow(&self) -> std::cell::Ref<'_, T> {
        self.inner.borrow()
    }

    /// Borrow the lock immutably (send build — wraps `RwLock::read`).
    #[cfg(feature = "send")]
    #[inline]
    pub fn borrow(&self) -> std::sync::RwLockReadGuard<'_, T> {
        self.inner.read().unwrap()
    }

    /// Borrow the lock mutably. The returned guard derefs to `&mut T`;
    /// callers use it identically to `RefCell::borrow_mut`.
    #[cfg(not(feature = "send"))]
    #[inline]
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, T> {
        self.inner.borrow_mut()
    }

    /// Borrow the lock mutably (send build — wraps `RwLock::write`).
    #[cfg(feature = "send")]
    #[inline]
    pub fn borrow_mut(&self) -> std::sync::RwLockWriteGuard<'_, T> {
        self.inner.write().unwrap()
    }
}
