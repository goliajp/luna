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

/// What CTR_EL0 says a code write needs before it can run: the data cache
/// line to clean to the point of unification, and the instruction cache
/// line to invalidate there. `None` means the core keeps that side coherent
/// by itself (IDC or DIC set).
#[derive(Debug, PartialEq, Eq)]
#[cfg(any(
    test,
    all(
        target_arch = "aarch64",
        not(target_vendor = "apple"),
        not(target_os = "windows")
    )
))]
struct CacheMaintenance {
    clean_line: Option<usize>,
    invalidate_line: Option<usize>,
}

#[cfg(any(
    test,
    all(
        target_arch = "aarch64",
        not(target_vendor = "apple"),
        not(target_os = "windows")
    )
))]
impl CacheMaintenance {
    fn from_ctr(ctr: u64) -> Self {
        // IDC (bit 28): no data cache clean needed; DminLine (19:16) is
        // log2 of the line size in 4-byte words
        let clean_line = (ctr & (1 << 28) == 0).then(|| 4usize << ((ctr >> 16) & 0xf));
        // DIC (bit 29): no instruction cache invalidation needed; IminLine (3:0)
        let invalidate_line = (ctr & (1 << 29) == 0).then(|| 4usize << (ctr & 0xf));
        Self {
            clean_line,
            invalidate_line,
        }
    }
}

/// Start addresses of the `line`-sized lines covering `[start, start + len)`.
#[cfg(any(
    test,
    all(
        target_arch = "aarch64",
        not(target_vendor = "apple"),
        not(target_os = "windows")
    )
))]
fn lines(start: usize, len: usize, line: usize) -> impl Iterator<Item = usize> {
    (start & !(line - 1)..start + len).step_by(line)
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
    let ctr: u64;
    // SAFETY: reading CTR_EL0 has no side effects; Linux lets EL0 read it
    unsafe { asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags)) };
    let m = CacheMaintenance::from_ctr(ctr);
    if let Some(line) = m.clean_line {
        for addr in lines(start, len, line) {
            // SAFETY: cache maintenance on a mapped address
            unsafe { asm!("dc cvau, {}", in(reg) addr, options(nostack, preserves_flags)) };
        }
    }
    // SAFETY: barrier only
    unsafe { asm!("dsb ish", options(nostack, preserves_flags)) };
    if let Some(line) = m.invalidate_line {
        for addr in lines(start, len, line) {
            // SAFETY: cache maintenance on a mapped address
            unsafe { asm!("ic ivau, {}", in(reg) addr, options(nostack, preserves_flags)) };
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

#[cfg(test)]
mod tests {
    use super::{CacheMaintenance, lines};

    const IDC: u64 = 1 << 28;
    const DIC: u64 = 1 << 29;
    // DminLine = IminLine = 4: 64-byte lines
    const LINES_64: u64 = (4 << 16) | 4;

    #[test]
    fn both_sides_needed_when_idc_and_dic_are_clear() {
        let m = CacheMaintenance::from_ctr(LINES_64);
        assert_eq!(m.clean_line, Some(64));
        assert_eq!(m.invalidate_line, Some(64));
    }

    #[test]
    fn idc_skips_the_clean() {
        let m = CacheMaintenance::from_ctr(LINES_64 | IDC);
        assert_eq!(m.clean_line, None);
        assert_eq!(m.invalidate_line, Some(64));
    }

    #[test]
    fn dic_skips_the_invalidate() {
        let m = CacheMaintenance::from_ctr(LINES_64 | DIC);
        assert_eq!(m.clean_line, Some(64));
        assert_eq!(m.invalidate_line, None);
    }

    #[test]
    fn idc_and_dic_skip_both() {
        let m = CacheMaintenance::from_ctr(LINES_64 | IDC | DIC);
        assert_eq!(
            m,
            CacheMaintenance {
                clean_line: None,
                invalidate_line: None
            }
        );
    }

    #[test]
    fn line_sizes_come_from_their_own_fields() {
        // Cortex-A72 reports CTR_EL0 = 0x8444c004: 64-byte lines both
        // sides, IDC and DIC clear
        let m = CacheMaintenance::from_ctr(0x8444_c004);
        assert_eq!(
            m,
            CacheMaintenance {
                clean_line: Some(64),
                invalidate_line: Some(64)
            }
        );
        // DminLine 3 (32 bytes), IminLine 5 (128 bytes)
        let m = CacheMaintenance::from_ctr((3 << 16) | 5);
        assert_eq!(
            m,
            CacheMaintenance {
                clean_line: Some(32),
                invalidate_line: Some(128)
            }
        );
    }

    #[test]
    fn lines_cover_an_unaligned_range() {
        assert_eq!(
            lines(0x1030, 0x20, 64).collect::<Vec<_>>(),
            [0x1000, 0x1040]
        );
        assert_eq!(lines(0x1000, 0x40, 64).collect::<Vec<_>>(), [0x1000]);
        assert_eq!(lines(0x103f, 1, 64).collect::<Vec<_>>(), [0x1000]);
    }

    // a page that already ran code is made writable, rewritten and made
    // executable again. the kernel syncs the caches only when a page is
    // first mapped, so here `sync_icache` alone decides whether the new
    // instructions run
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    #[test]
    fn rewritten_code_in_a_page_that_already_ran_runs_new_instructions() {
        use super::sync_icache;
        use std::ffi::c_void;

        unsafe extern "C" {
            fn mmap(
                addr: *mut c_void,
                len: usize,
                prot: i32,
                flags: i32,
                fd: i32,
                off: i64,
            ) -> *mut c_void;
            fn mprotect(addr: *mut c_void, len: usize, prot: i32) -> i32;
            fn munmap(addr: *mut c_void, len: usize) -> i32;
        }
        const PROT_READ: i32 = 1;
        const PROT_WRITE: i32 = 2;
        const PROT_EXEC: i32 = 4;
        const MAP_PRIVATE: i32 = 2;
        const MAP_ANONYMOUS: i32 = 0x20;
        const PAGE: usize = 4096;
        const RET: u32 = 0xd65f_03c0;

        // SAFETY: a fresh private anonymous mapping
        let page = unsafe {
            mmap(
                std::ptr::null_mut(),
                PAGE,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(page as isize, -1, "mmap");
        let code = page.cast::<u32>();
        let mut stale = Vec::new();
        for i in 0..200_000u32 {
            let k = i & 0xffff;
            // SAFETY: the page is ours; it is writable between the mprotect calls
            unsafe {
                assert_eq!(
                    mprotect(page, PAGE, PROT_READ | PROT_WRITE),
                    0,
                    "mprotect rw"
                );
                // movz w0, #k; ret
                code.write_volatile(0x5280_0000 | (k << 5));
                code.add(1).write_volatile(RET);
                sync_icache(page as usize, 8);
                assert_eq!(
                    mprotect(page, PAGE, PROT_READ | PROT_EXEC),
                    0,
                    "mprotect rx"
                );
            }
            // SAFETY: the page holds a complete function returning a u32
            let f: extern "C" fn() -> u32 = unsafe { std::mem::transmute(page) };
            let got = f();
            if got != k {
                stale.push((i, k, got));
            }
        }
        // SAFETY: mapped above
        unsafe { munmap(page, PAGE) };
        assert!(
            stale.is_empty(),
            "stale code ran {} times: {:?}",
            stale.len(),
            &stale[..stale.len().min(5)]
        );
    }
}
