// own binary: the counting allocator and the process memory counters must
// see no other test's allocations

//! The machine code the method and trace JIT compile for a `Vm` is freed
//! when that `Vm` drops: creating and dropping many `Vm`s that each
//! compile functions and traces must not grow the live bytes of the global
//! allocator, the executable mappings of the process (Linux, macOS), nor, on
//! Windows, the memory committed to the process.

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
        // SAFETY: `GlobalAlloc::alloc`'s contract, passed through
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc::alloc_zeroed`'s contract, passed through
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was allocated by this allocator, which got it
        // from `System` with the same `layout`
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` was allocated by this allocator, which got it
        // from `System` with `layout`; `new_size` is the caller's, under
        // `GlobalAlloc::realloc`'s contract
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

/// Bytes of the process's address space mapped executable.
#[cfg(target_os = "linux")]
fn executable_bytes() -> usize {
    let maps = std::fs::read_to_string("/proc/self/maps").expect("read /proc/self/maps");
    let mut total = 0;
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(range), Some(perms)) = (fields.next(), fields.next()) else {
            continue;
        };
        if perms.as_bytes().get(2) != Some(&b'x') {
            continue;
        }
        let (lo, hi) = range.split_once('-').expect("address range");
        let lo = usize::from_str_radix(lo, 16).expect("start address");
        let hi = usize::from_str_radix(hi, 16).expect("end address");
        total += hi - lo;
    }
    total
}

/// Bytes of the process's address space mapped executable.
#[cfg(target_os = "macos")]
fn executable_bytes() -> usize {
    // vm_region_basic_info_64 is declared with #pragma pack(4)
    #[repr(C, packed(4))]
    #[derive(Default)]
    struct BasicInfo64 {
        protection: i32,
        max_protection: i32,
        inheritance: u32,
        shared: u32,
        reserved: u32,
        offset: u64,
        behavior: i32,
        user_wired_count: u16,
    }
    const VM_REGION_BASIC_INFO_64: i32 = 9;
    const VM_PROT_EXECUTE: i32 = 4;
    unsafe extern "C" {
        static mach_task_self_: u32;
        fn mach_vm_region(
            task: u32,
            address: *mut u64,
            size: *mut u64,
            flavor: i32,
            info: *mut i32,
            count: *mut u32,
            object_name: *mut u32,
        ) -> i32;
    }
    let mut total = 0;
    let mut addr: u64 = 0;
    loop {
        let mut size: u64 = 0;
        let mut info = BasicInfo64::default();
        let mut count = (std::mem::size_of::<BasicInfo64>() / 4) as u32;
        let mut object = 0u32;
        // SAFETY: `info` is a vm_region_basic_info_64 of `count` 4-byte
        // words; mach_vm_region only reads the task's address space
        let kr = unsafe {
            mach_vm_region(
                mach_task_self_,
                &mut addr,
                &mut size,
                VM_REGION_BASIC_INFO_64,
                (&raw mut info).cast::<i32>(),
                &mut count,
                &mut object,
            )
        };
        if kr != 0 {
            break;
        }
        if info.protection & VM_PROT_EXECUTE != 0 {
            total += size as usize;
        }
        addr += size;
    }
    assert!(total > 0, "mach_vm_region found no executable memory");
    total
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn dropping_a_vm_unmaps_its_code_pages() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for _ in 0..5 {
        one_vm();
    }
    let before = executable_bytes();
    let (mut chunks, mut dispatched) = (0, 0);
    const N: usize = 200;
    for _ in 0..N {
        let (c, d) = one_vm();
        chunks += c;
        dispatched += d;
    }
    let grown = executable_bytes() as isize - before as isize;
    assert!(chunks > 0, "the method JIT compiled nothing");
    assert!(dispatched > 0, "no trace was dispatched");
    // each Vm maps at least a page of code, so a leak grows by N pages
    assert!(
        grown < 64 * 1024,
        "{grown} bytes still mapped executable after {N} Vms were dropped"
    );
}

/// Runs `SRC` on a fresh Vm of `engine`; returns (traces it installed
/// from the engine, traces dispatched).
fn engine_vm(engine: &luna_jit::Engine) -> (u64, u64) {
    let mut vm = engine.new_vm(luna_jit::LuaVersion::Lua54);
    vm.jit.trace_hot_threshold = 2;
    vm.jit.call_hot_threshold = 2;
    vm.eval(SRC).expect("eval");
    (vm.trace_adopted_count(), vm.trace_dispatched_count())
}

/// Vms that install an engine's traces copy the code into their own
/// memory: dropping them frees it, and the engine holds the same bytes
/// however many Vms took its traces.
#[test]
fn vms_of_an_engine_free_the_code_they_installed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let engine = luna_jit::Engine::new();
    for _ in 0..5 {
        engine_vm(&engine);
    }
    let (held, traces) = (engine.bytes(), engine.trace_count());
    let before = LIVE.load(Ordering::Relaxed);
    let (mut adopted, mut dispatched) = (0, 0);
    const N: usize = 200;
    for _ in 0..N {
        let (a, d) = engine_vm(&engine);
        adopted += a;
        dispatched += d;
    }
    let grown = LIVE.load(Ordering::Relaxed) - before;
    assert!(adopted >= N as u64, "the Vms installed {adopted} traces");
    assert!(dispatched > 0, "no trace was dispatched");
    assert_eq!(
        (engine.bytes(), engine.trace_count()),
        (held, traces),
        "the engine grew"
    );
    assert!(
        grown < 64 * 1024,
        "{grown} bytes still live after {N} Vms were dropped"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn vms_of_an_engine_unmap_the_code_they_installed() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let engine = luna_jit::Engine::new();
    for _ in 0..5 {
        engine_vm(&engine);
    }
    let before = executable_bytes();
    let mut adopted = 0;
    const N: usize = 200;
    for _ in 0..N {
        adopted += engine_vm(&engine).0;
    }
    let grown = executable_bytes() as isize - before as isize;
    assert!(adopted >= N as u64, "the Vms installed {adopted} traces");
    assert!(
        grown < 64 * 1024,
        "{grown} bytes still mapped executable after {N} Vms were dropped"
    );
}
