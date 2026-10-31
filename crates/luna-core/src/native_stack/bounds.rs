//! The running thread's stack bounds, as each OS gives them.

#[cfg(all(any(target_os = "linux", target_os = "android"), not(miri)))]
pub(super) mod os {
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

    pub(in crate::native_stack) fn stack_low() -> Option<usize> {
        if let Some(low) = super::main_thread_low() {
            return Some(low);
        }
        stack_bounds().map(|(addr, _)| addr)
    }

    /// The running thread's stack as the C library reports it: its lowest
    /// address and its size.
    pub(super) fn stack_bounds() -> Option<(usize, usize)> {
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
            (r == 0).then_some((addr as usize, size))
        }
    }
}

/// The main thread's stack on glibc: its top (`__libc_stack_end`) less the
/// stack size limit. `pthread_getattr_np` would read `/proc/self/maps`
/// for it, allocating and freeing a stream buffer on the C heap, which
/// moves where everything allocated after it lands; in a benchmark that
/// alone made the interpreter's call loop 9% slower.
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    target_env = "gnu",
    not(miri)
))]
fn main_thread_low() -> Option<usize> {
    use std::ffi::c_void;
    const RLIMIT_STACK: i32 = 3;
    unsafe extern "C" {
        static __libc_stack_end: *const c_void;
        fn getpid() -> i32;
        fn gettid() -> i32;
        fn getrlimit(resource: i32, rlim: *mut [u64; 2]) -> i32;
    }
    let mut rlim = [0u64; 2];
    // SAFETY: the C library's own process and thread ids, its record of
    // where the main thread's stack starts (set before `main` runs), and
    // the stack size limit written to a local of `struct rlimit`'s layout
    let (main, top, r) = unsafe {
        (
            gettid() == getpid(),
            __libc_stack_end as usize,
            getrlimit(RLIMIT_STACK, &mut rlim),
        )
    };
    if !main || r != 0 || rlim[0] == u64::MAX {
        return None;
    }
    top.checked_sub(rlim[0] as usize)
}

/// The main thread's stack on musl: its top less the stack size limit.
/// musl's `pthread_getattr_np` gives the main thread only the part of its
/// stack the kernel has mapped so far (about 100 KiB below the stack
/// pointer at start), which the stack grows past on demand; the top it
/// gives is right.
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    target_env = "musl",
    not(miri)
))]
fn main_thread_low() -> Option<usize> {
    const RLIMIT_STACK: i32 = 3;
    unsafe extern "C" {
        fn getpid() -> i32;
        fn gettid() -> i32;
        fn getrlimit(resource: i32, rlim: *mut [u64; 2]) -> i32;
    }
    let mut rlim = [0u64; 2];
    // SAFETY: the C library's own process and thread ids, and the stack
    // size limit written to a local of `struct rlimit`'s layout
    let (main, r) = unsafe { (gettid() == getpid(), getrlimit(RLIMIT_STACK, &mut rlim)) };
    if !main || r != 0 || rlim[0] == u64::MAX {
        return None;
    }
    let (addr, size) = os::stack_bounds()?;
    (addr + size).checked_sub(rlim[0] as usize)
}

#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    not(any(target_env = "gnu", target_env = "musl")),
    not(miri)
))]
fn main_thread_low() -> Option<usize> {
    None
}

#[cfg(all(target_vendor = "apple", not(miri)))]
pub(super) mod os {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn pthread_self() -> *mut c_void;
        fn pthread_get_stackaddr_np(thread: *mut c_void) -> *mut c_void;
        fn pthread_get_stacksize_np(thread: *mut c_void) -> usize;
    }

    pub(in crate::native_stack) fn stack_low() -> Option<usize> {
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

#[cfg(all(windows, not(miri)))]
pub(super) mod os {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadStackLimits(low: *mut usize, high: *mut usize);
    }

    pub(in crate::native_stack) fn stack_low() -> Option<usize> {
        let (mut low, mut high) = (0usize, 0usize);
        // SAFETY: writes the running thread's stack bounds to two locals
        unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
        Some(low)
    }
}

// Miri provides no `__libc_stack_end`, and the addresses it gives locals
// lie inside no stack bounds an OS call could report, so under it the
// bounds are unknown and only the call counts apply
#[cfg(any(
    miri,
    not(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        windows
    ))
))]
pub(super) mod os {
    pub(in crate::native_stack) fn stack_low() -> Option<usize> {
        None
    }
}
