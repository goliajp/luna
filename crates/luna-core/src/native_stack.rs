//! How much of the running thread's native stack is left.
//!
//! Lua-to-Lua calls in the interpreter push frames on the VM's own stack,
//! but some nesting runs on the native stack: a library function calling
//! back into Lua (`table.sort`'s comparator, `string.gsub`'s replacement,
//! `tostring`'s `__tostring`), a coroutine resume, the parser, and
//! compiled code calling itself. PUC bounds these by counting C calls;
//! luna counts them the same way, but a count does not say how many bytes
//! a level takes, and a thread with a small stack runs out first. So each
//! such entry also compares the stack pointer with the thread's real stack
//! bounds, which are read from the OS once per thread.
//!
//! Where the bounds cannot be read (other targets, or code running on a
//! stack the thread did not start with), nothing is reported as low and
//! only the counts apply.

use std::cell::Cell;

/// Bytes a nested entry leaves free below it: enough to raise the error,
/// unwind, and run a message handler and the library code it calls.
pub const RESERVE: usize = 96 * 1024;

/// What an error handler may still use of [`RESERVE`] (PUC gives a
/// handler a few more C levels than the code that failed).
pub const HANDLER_RESERVE: usize = RESERVE / 2;

/// Bytes compiled code leaves free before it hands a self-recursive call
/// to the interpreter: more than [`RESERVE`] by the room that handing over
/// and the interpreter's own entry take, so the handed-over call does not
/// itself fail for want of stack.
pub const JIT_RESERVE: usize = 2 * RESERVE;

/// Bytes that must be free to compile a function or a trace: Cranelift
/// runs on the caller's stack, and an unoptimised build of it needs a few
/// hundred kilobytes. With less left the code is not compiled now, and
/// may be on a later call.
pub const COMPILE_RESERVE: usize = if cfg!(debug_assertions) {
    512 * 1024
} else {
    JIT_RESERVE
};

thread_local! {
    /// lowest address of this thread's stack; 0 before it is read, 1 when
    /// it cannot be
    static LOW: Cell<usize> = const { Cell::new(0) };
}

/// The address of a local in the caller's frame: the stack pointer, near
/// enough for a check that keeps tens of kilobytes free.
#[inline(always)]
pub fn sp() -> usize {
    let b = 0u8;
    std::hint::black_box(&b) as *const u8 as usize
}

/// The lowest address of the running thread's stack, or 1 when it is not
/// known.
#[inline]
pub fn low() -> usize {
    let low = LOW.with(Cell::get);
    if low != 0 {
        return low;
    }
    let low = os::stack_low().filter(|&l| l > 1).unwrap_or(1);
    LOW.with(|c| c.set(low));
    low
}

/// The stack address below which compiled code stops recursing natively
/// (see [`JIT_RESERVE`]), or 0 when the bounds are not known.
pub fn jit_limit() -> usize {
    match low() {
        1 => 0,
        low => low + JIT_RESERVE,
    }
}

/// Whether fewer than `reserve` bytes of the stack are left. A stack
/// pointer outside the thread's stack (an embedder running luna on a
/// stack of its own) is never low.
#[inline]
pub fn is_low(reserve: usize) -> bool {
    sp().wrapping_sub(low()) < reserve
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use std::ffi::c_void;

    /// `pthread_attr_t` is 56 bytes or fewer on every Linux libc luna
    /// builds for (64 on glibc aarch64); this is room for all of them.
    #[repr(C, align(16))]
    struct Attr([u8; 128]);

    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_getattr_np(thread: usize, attr: *mut Attr) -> i32;
        fn pthread_attr_getstack(
            attr: *const Attr,
            addr: *mut *mut c_void,
            size: *mut usize,
        ) -> i32;
        fn pthread_attr_destroy(attr: *mut Attr) -> i32;
    }

    pub(super) fn stack_low() -> Option<usize> {
        let mut attr = Attr([0; 128]);
        let mut addr: *mut c_void = std::ptr::null_mut();
        let mut size = 0usize;
        // SAFETY: `attr` is larger and more aligned than the libc's
        // `pthread_attr_t`; getattr_np initialises it on success, and only
        // then is it read and destroyed; `addr` and `size` are locals
        unsafe {
            if pthread_getattr_np(pthread_self(), &mut attr) != 0 {
                return None;
            }
            let r = pthread_attr_getstack(&attr, &mut addr, &mut size);
            pthread_attr_destroy(&mut attr);
            (r == 0).then_some(addr as usize)
        }
    }
}

#[cfg(target_vendor = "apple")]
mod os {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn pthread_self() -> *mut c_void;
        fn pthread_get_stackaddr_np(thread: *mut c_void) -> *mut c_void;
        fn pthread_get_stacksize_np(thread: *mut c_void) -> usize;
    }

    pub(super) fn stack_low() -> Option<usize> {
        // SAFETY: both read the running thread's own bookkeeping, which
        // lives as long as the thread
        let (top, size) = unsafe {
            let t = pthread_self();
            (
                pthread_get_stackaddr_np(t) as usize,
                pthread_get_stacksize_np(t),
            )
        };
        top.checked_sub(size)
    }
}

#[cfg(windows)]
mod os {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }

    pub(super) fn stack_low() -> Option<usize> {
        let (mut low, mut high) = (0usize, 0usize);
        // SAFETY: writes the running thread's stack bounds to two locals
        unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
        Some(low)
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
mod os {
    pub(super) fn stack_low() -> Option<usize> {
        None
    }
}
