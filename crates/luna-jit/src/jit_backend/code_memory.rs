//! JIT memory provider that makes newly written code visible to the
//! instruction fetch path before it runs.
//!
//! Cranelift's `SystemMemoryProvider` invalidates the instruction cache
//! on aarch64 with a fixed 64-byte line and without cleaning the data
//! cache first. On cores that report CTR_EL0.IDC == 0 the architecture
//! requires that clean, so the flush is only correct there because the
//! pages happen to be freshly mapped and the kernel syncs them on their
//! first executable mapping. A `Vm` frees its code when it drops and the
//! next compile writes new code, so luna does not rely on that: it syncs
//! every code range itself, with line sizes read from CTR_EL0.

use cranelift_jit::{BranchProtection, JITMemoryKind, JITMemoryProvider, SystemMemoryProvider};
use cranelift_module::ModuleResult;
use std::io;

pub(crate) struct CodeMemory {
    inner: SystemMemoryProvider,
    /// Code ranges allocated since the last finalize.
    unsynced: Vec<(usize, usize)>,
}

impl CodeMemory {
    pub(crate) fn new() -> Self {
        Self {
            inner: SystemMemoryProvider::new(),
            unsynced: Vec::new(),
        }
    }
}

impl JITMemoryProvider for CodeMemory {
    fn allocate(&mut self, size: usize, align: u64, kind: JITMemoryKind) -> io::Result<*mut u8> {
        let exec = matches!(kind, JITMemoryKind::Executable);
        let ptr = self.inner.allocate(size, align, kind)?;
        if exec {
            self.unsynced.push((ptr as usize, size));
        }
        Ok(ptr)
    }

    unsafe fn free_memory(&mut self) {
        self.unsynced.clear();
        // SAFETY: forwarded from the caller
        unsafe { self.inner.free_memory() }
    }

    fn finalize(&mut self, branch_protection: BranchProtection) -> ModuleResult<()> {
        // relocations are already applied when the module finalizes its
        // memory, so the code bytes are final here. the inner finalize then
        // flushes every core's pipeline
        for (start, len) in self.unsynced.drain(..) {
            // SAFETY: the range was handed out by `allocate` for executable memory and is
            // still allocated and readable
            unsafe { sync_icache(start, len) };
        }
        self.inner.finalize(branch_protection)
    }
}

#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
unsafe fn sync_icache(start: usize, len: usize) {
    unsafe extern "C" {
        fn sys_icache_invalidate(start: *mut std::ffi::c_void, len: usize);
    }
    // SAFETY: the caller guarantees the range is mapped
    unsafe { sys_icache_invalidate(start as *mut std::ffi::c_void, len) }
}

// the sequence compiler-rt's __clear_cache uses: clean the data cache to
// the point of unification, then invalidate the instruction cache there.
// both are broadcast to every core in the inner shareable domain
#[cfg(all(
    target_arch = "aarch64",
    not(target_vendor = "apple"),
    not(target_os = "windows")
))]
unsafe fn sync_icache(start: usize, len: usize) {
    use std::arch::asm;
    let end = start + len;
    let ctr: u64;
    // SAFETY: reading CTR_EL0 has no side effects; Linux lets EL0 read it
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    // CTR_EL0.IDC: data cache clean not required for coherence
    if ctr & (1 << 28) == 0 {
        let line = 4usize << ((ctr >> 16) & 0xf);
        let mut addr = start & !(line - 1);
        while addr < end {
            // SAFETY: cache maintenance on a mapped address
            unsafe { asm!("dc cvau, {}", in(reg) addr, options(nostack, preserves_flags)) };
            addr += line;
        }
    }
    // SAFETY: barrier only
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    // CTR_EL0.DIC: instruction cache invalidation not required
    if ctr & (1 << 29) == 0 {
        let line = 4usize << (ctr & 0xf);
        let mut addr = start & !(line - 1);
        while addr < end {
            // SAFETY: cache maintenance on a mapped address
            unsafe { asm!("ic ivau, {}", in(reg) addr, options(nostack, preserves_flags)) };
            addr += line;
        }
        // SAFETY: barrier only
        unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    }
    // SAFETY: barrier only
    unsafe { asm!("isb", options(nostack, preserves_flags)) };
}

// x86 keeps the instruction cache coherent with stores; windows is
// handled by cranelift, which calls FlushInstructionCache there
#[cfg(not(all(target_arch = "aarch64", not(target_os = "windows"))))]
unsafe fn sync_icache(_start: usize, _len: usize) {}
