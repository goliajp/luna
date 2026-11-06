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
//! Where the bounds cannot be read (other targets, Miri, or code running
//! on a stack the thread did not start with), nothing is reported as low and
//! only the counts apply.
//!
//! A reserve must cover the deepest stack the code uses between two
//! checks: from a nested entry that found enough left down to the next
//! entry that checks, through the interpreter, a library function and a
//! collection step it runs. Otherwise the stack overflows before any
//! check can report it. That depth depends on the build: an unoptimised
//! build keeps every local of the interpreter loop in a slot of its own,
//! and AddressSanitizer pads each one, so the reserves are larger there.

use std::cell::Cell;

/// Bytes a nested entry leaves free below it: enough to raise the error,
/// unwind, and run a message handler and the library code it calls. An
/// optimised build needs a few kilobytes of it between two checks; an
/// unoptimised one about 60 KiB, 150 KiB with AddressSanitizer.
pub const RESERVE: usize = if cfg!(debug_assertions) {
    512 * 1024
} else {
    96 * 1024
};

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

/// The stack pointer. Read from its register where luna knows the
/// instruction: the address of a local can be elsewhere (AddressSanitizer
/// moves locals whose address is taken to a stack of its own, on the heap).
/// Elsewhere that address, near enough for a check that keeps tens of
/// kilobytes free.
#[inline(always)]
pub fn sp() -> usize {
    #[cfg(all(target_arch = "x86_64", not(miri)))]
    {
        let sp: usize;
        // SAFETY: copies the stack pointer register to `sp`; touches no memory
        unsafe {
            std::arch::asm!("mov {}, rsp", out(reg) sp, options(nomem, nostack, preserves_flags))
        };
        sp
    }
    #[cfg(all(target_arch = "aarch64", not(miri)))]
    {
        let sp: usize;
        // SAFETY: copies the stack pointer register to `sp`; touches no memory
        unsafe {
            std::arch::asm!("mov {}, sp", out(reg) sp, options(nomem, nostack, preserves_flags))
        };
        sp
    }
    #[cfg(any(not(any(target_arch = "x86_64", target_arch = "aarch64")), miri))]
    {
        let b = 0u8;
        std::ptr::addr_of!(b) as usize
    }
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

thread_local! {
    /// a stack address where [`nested_call_low`] found [`RESERVE`] left:
    /// a call that starts no deeper has at least as much
    static NOT_LOW_AT: Cell<usize> = const { Cell::new(usize::MAX) };
}

/// [`is_low`]`(`[`RESERVE`]`)` for a nested call, reading the bounds only
/// when the call starts deeper than one that found the stack not low. The
/// stack grows down, so any call above that point has more room; each level
/// of an unbounded nesting starts deeper, so it is checked.
#[inline(always)]
pub fn nested_call_low() -> bool {
    let sp = sp();
    #[cfg(test)]
    tests::checked_at(sp);
    if sp >= NOT_LOW_AT.with(Cell::get) {
        return false;
    }
    let low = is_low(RESERVE);
    if !low {
        NOT_LOW_AT.with(|c| c.set(sp));
    }
    low
}

/// Whether fewer than `reserve` bytes of the stack are left. A stack
/// pointer outside the thread's stack (an embedder running luna on a
/// stack of its own) is never low.
#[inline]
pub fn is_low(reserve: usize) -> bool {
    let sp = sp();
    #[cfg(test)]
    tests::checked_at(sp);
    sp.wrapping_sub(low()) < reserve
}

mod bounds;
use bounds::os;

#[cfg(test)]
pub(crate) mod tests;
