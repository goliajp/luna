// own binary: the counting allocator and the process memory counters must
// see no other test's allocations

//! The machine code the method and trace JIT compile for a `Vm` is freed
//! when that `Vm` drops: creating and dropping many `Vm`s that each
//! compile functions and traces must not grow the live bytes of the global
//! allocator, nor, on Windows, the memory committed to the process.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

// the tests measure the whole process, so they take turns
static SERIAL: Mutex<()> = Mutex::new(());

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards to `System`, only counting sizes
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            LIVE.fetch_add(
                new_size as isize - layout.size() as isize,
                Ordering::Relaxed,
            );
        }
        p
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const SRC: &str = r#"
    local function add(a, b) return a + b end
    local t, s = {}, 0
    for i = 1, 200 do t[i] = i end
    for i = 1, 200 do s = add(s, t[i]) end
    local w = 0
    while w < 200 do w = w + 1 s = s + w % 7 end
    return s
"#;

/// Runs `SRC` on a fresh JIT Vm; returns (method chunks, traces dispatched).
fn one_vm() -> (usize, u64) {
    let mut vm = luna_jit::new_with_jit(luna_jit::LuaVersion::Lua54);
    vm.jit.trace_hot_threshold = 2;
    vm.jit.call_hot_threshold = 2;
    vm.eval(SRC).expect("eval");
    (
        luna_jit::jit::cache_entry_count(&vm),
        vm.trace_dispatched_count(),
    )
}

#[test]
fn dropping_a_vm_frees_its_compiled_code() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // the first Vms fill one-time caches (host ISA, lazily built tables)
    for _ in 0..5 {
        one_vm();
    }
    let before = LIVE.load(Ordering::Relaxed);
    let (mut chunks, mut dispatched) = (0, 0);
    const N: usize = 200;
    for _ in 0..N {
        let (c, d) = one_vm();
        chunks += c;
        dispatched += d;
    }
    let grown = LIVE.load(Ordering::Relaxed) - before;
    assert!(chunks > 0, "the method JIT compiled nothing");
    assert!(dispatched > 0, "no trace was dispatched");
    // each Vm maps at least a page of code, so a leak grows by N pages
    assert!(
        grown < 64 * 1024,
        "{grown} bytes still live after {N} Vms were dropped"
    );
}

/// Bytes of the process's memory: `PrivateUsage` from
/// `GetProcessMemoryInfo`; from `VirtualQuery`, every committed region of
/// the address space and the mapped views among them.
#[cfg(windows)]
#[derive(Clone, Copy, Debug)]
struct ProcessMemory {
    private: isize,
    committed: isize,
    mapped: isize,
}

#[cfg(windows)]
impl ProcessMemory {
    fn now() -> Self {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_MAPPED, MEMORY_BASIC_INFORMATION, VirtualQuery,
        };
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: `counters` is a PROCESS_MEMORY_COUNTERS_EX of `size` bytes
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
                size,
            )
        };
        assert_ne!(
            ok,
            0,
            "GetProcessMemoryInfo: {}",
            std::io::Error::last_os_error()
        );

        let (mut committed, mut mapped) = (0, 0);
        let mut addr = 0usize;
        loop {
            let mut region = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: VirtualQuery only reads the address space; it returns 0
            // past the highest user address
            let n = unsafe {
                VirtualQuery(
                    addr as *const std::ffi::c_void,
                    &mut region,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if n == 0 {
                break;
            }
            if region.State == MEM_COMMIT {
                committed += region.RegionSize;
                if region.Type == MEM_MAPPED {
                    mapped += region.RegionSize;
                }
            }
            addr = region.BaseAddress as usize + region.RegionSize;
        }
        assert!(committed > 0, "VirtualQuery found no committed memory");
        ProcessMemory {
            private: counters.PrivateUsage as isize,
            committed: committed as isize,
            mapped: mapped as isize,
        }
    }

    fn growth_since(self, before: Self) -> Self {
        ProcessMemory {
            private: self.private - before.private,
            committed: self.committed - before.committed,
            mapped: self.mapped - before.mapped,
        }
    }
}

#[cfg(windows)]
#[test]
fn dropping_a_vm_returns_its_code_pages_to_windows() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..5 {
        one_vm();
    }
    let before = ProcessMemory::now();
    let (mut chunks, mut dispatched) = (0, 0);
    const N: usize = 200;
    for _ in 0..N {
        let (c, d) = one_vm();
        chunks += c;
        dispatched += d;
    }
    let grown = ProcessMemory::now().growth_since(before);
    assert!(chunks > 0, "the method JIT compiled nothing");
    assert!(dispatched > 0, "no trace was dispatched");
    eprintln!("after {N} Vms the process grew by {grown:?} bytes");
    // each Vm commits at least a page of code, so a leak grows by N pages
    // (800 KiB). Cranelift maps the code as a section, which PrivateUsage
    // does not count; the heap may keep a few hundred KiB it grew into
    assert!(
        grown.mapped < 64 * 1024 && grown.committed < 1024 * 1024 && grown.private < 1024 * 1024,
        "memory still committed after {N} Vms were dropped: {grown:?}"
    );
}
