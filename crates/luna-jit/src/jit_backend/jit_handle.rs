use super::*;

/// Owns the JIT module + holds the entry fn ptr alive for the
/// lifetime of the executable mmap. Drop deallocates the mmap.
///
/// `_module` is typed as
/// [`SendJitModule`] (the `Send` sleeve newtype) so the module's
/// `Send` story stays type-system-asserted at this field. The
/// wrapper is a `#[repr(Rust)]` newtype with `Deref<Target = JITModule>`
/// + `DerefMut`, so existing call sites that touched
/// `handle._module.<method>` keep working transparently. The wrapper
/// also gates `Send` for any future container that wants to hold a
/// `JitHandle`; today the handle itself stays `!Send` because
/// `entry_raw: *const u8` is `!Send`; the manual `Send` impl below
/// builds on the module sleeve.
pub struct JitHandle {
    pub(super) _module: SendJitModule,
    pub(super) entry_raw: *const u8,
    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    /// Picks the right `extern "C"` fn-type to transmute to at the
    /// call site.
    pub(super) num_args: u8,
    /// True when the Lua chunk this fn was lowered from contains a
    /// `Return1`; false when only `Return0` is present. Drives the
    /// dispatch wrap (Int wrap vs empty Vec).
    pub(super) returns_one: bool,
    /// bit `i = 1` ↔ arg slot `i` is f64 (passed as i64
    /// bit-pattern across the ABI, bitcast inside the JIT). Bits
    /// ≥ MAX_JIT_ARITY are always zero.
    pub(super) arg_float_mask: u8,
    /// bit `i = 1` ↔ arg slot `i` is `Gc<Table>` raw ptr.
    /// Mutually exclusive with `arg_float_mask` for the same bit.
    pub(super) arg_table_mask: u8,
    /// true iff the Proto's `Return1` value is f64.
    /// Meaningful only when `returns_one == true`.
    pub(super) ret_is_float: bool,
    /// true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. Mutually exclusive with `ret_is_float`.
    pub(super) ret_is_table: bool,
}

// sibling of the always-on
// `unsafe impl Send for TraceHandle` in `trace.rs`. JitHandle
// holds the same shape: a `SendJitModule` (Send via its wrapper,
// see `send_jit_module.rs`) plus an `entry_raw: *const u8` raw
// fn pointer addressing mcode owned by `_module`. The raw pointer
// is `!Send` by default — this manual impl is the explicit lift.
//
// SAFETY: each field is safely Send:
//   - `_module: SendJitModule` — Send via the `unsafe impl Send
//     for SendJitModule` in `send_jit_module.rs`. luna only
//     constructs `JITModule` with `SystemMemoryProvider` (Send,
//     per cranelift-jit's `memory/system.rs:126`).
//   - `entry_raw: *const u8` — addresses mcode in `_module`'s
//     mmap'd page. Because `_module` ships with the handle (the
//     handle owns it by-value), the pointer remains
//     dereferenceable on whichever thread the handle lands on.
//     Read-only on the hot path (transmuted to `extern "C"` fn,
//     called). No aliasing.
//   - remaining fields are primitive scalars.
//
// Cross-thread dispatch is gated separately on the
// `scoped_jit_vm_rebind` RAII (per-`enter_jit` TLS install +
// restore), which works on any OS thread because the TLS slot is
// captured-and-restored at function scope rather than statically
// pinned.
unsafe impl Send for JitHandle {}

impl JitHandle {
    /// Frees the compiled code.
    ///
    /// # Safety
    ///
    /// The entry point is not running and will not be called again.
    pub(crate) unsafe fn free(self) {
        // SAFETY: forwarded from the caller
        unsafe { self._module.free() }
    }

    /// Invoke the entry with zero args. Panics in debug if the
    /// compiled Proto had `num_args > 0`.
    #[inline]
    pub fn call(&self) -> i64 {
        debug_assert_eq!(
            self.num_args, 0,
            "JitHandle::call() is the zero-arg form; use call_with for higher arity"
        );
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        let f: IntChunkFn = unsafe { std::mem::transmute(self.entry_raw) };
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        unsafe { f() }
    }

    /// Invoke the entry with a slice of i64 args. Length must match
    /// `num_args`; the dispatcher picks the right `extern "C"` fn
    /// shape and transmutes at the call site.
    pub fn call_with(&self, args: &[i64]) -> i64 {
        debug_assert_eq!(args.len(), self.num_args as usize);
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        unsafe {
            match self.num_args {
                0 => (std::mem::transmute::<*const u8, IntChunkFn>(self.entry_raw))(),
                1 => (std::mem::transmute::<*const u8, IntFn1>(self.entry_raw))(args[0]),
                2 => (std::mem::transmute::<*const u8, IntFn2>(self.entry_raw))(args[0], args[1]),
                3 => (std::mem::transmute::<*const u8, IntFn3>(self.entry_raw))(
                    args[0], args[1], args[2],
                ),
                4 => (std::mem::transmute::<*const u8, IntFn4>(self.entry_raw))(
                    args[0], args[1], args[2], args[3],
                ),
                _ => unreachable!("MAX_JIT_ARITY enforces num_args <= 4"),
            }
        }
    }

    /// Raw entry fn ptr. The dispatcher stashes a copy in `Proto.jit` so the
    /// dispatch hot-path doesn't have to borrow back through the
    /// handle on every call. The handle itself stays parked in
    /// `Vm.jit_handles` to keep the mmap alive.
    #[inline]
    pub fn entry_raw(&self) -> *const u8 {
        self.entry_raw
    }

    /// `#[doc(hidden)]` accessor returning
    /// the parked `_module` borrowed at the `SendJitModule` newtype.
    /// Lets the regression test
    /// (`tests/it/jit_vm_scoped_rebind.rs`) statically assert the
    /// field type is the `Send` sleeve. The borrow checker enforces the
    /// type match at this fn's signature — if `_module` ever degrades
    /// to bare `JITModule` again, this signature stops compiling.
    #[doc(hidden)]
    #[inline]
    pub fn __send_module(&self) -> &SendJitModule {
        &self._module
    }

    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    #[inline]
    pub fn num_args(&self) -> u8 {
        self.num_args
    }

    /// True when the Lua chunk this fn was lowered from ends in
    /// `Return1` (so its result is a single Lua value). False
    /// means the chunk only side-effects + `Return0`; the dispatch
    /// layer should hand the host an empty `Vec<Value>`.
    #[inline]
    pub fn returns_one(&self) -> bool {
        self.returns_one
    }

    /// packed Float-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is f64 (the dispatcher passes `f64::to_bits` packed into the
    /// i64 ABI slot).
    #[inline]
    pub fn arg_float_mask(&self) -> u8 {
        self.arg_float_mask
    }

    /// true iff the Proto's `Return1` value is f64. The
    /// dispatcher wraps the i64 ABI return as `Value::Float(
    /// f64::from_bits(r))` when set, `Value::Int(r)` otherwise.
    #[inline]
    pub fn ret_is_float(&self) -> bool {
        self.ret_is_float
    }

    /// packed Table-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is `Gc<Table>` (the dispatcher passes the raw `as_ptr() as
    /// i64` value).
    #[inline]
    pub fn arg_table_mask(&self) -> u8 {
        self.arg_table_mask
    }

    /// true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. The dispatcher wraps the i64 ABI return
    /// as `Value::Table(Gc::from_ptr(r as *mut Table))`.
    #[inline]
    pub fn ret_is_table(&self) -> bool {
        self.ret_is_table
    }
}
