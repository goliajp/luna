// own binary: the counting allocator and the process memory counters must
// see no other test's allocations

//! The machine code the method and trace JIT compile for a `Vm` is freed
//! when that `Vm` drops: creating and dropping many `Vm`s that each
//! compile functions and traces must not grow the live bytes of the global
//! allocator, the executable mappings of the process (Linux, macOS), nor, on
//! Windows, the memory committed to the process. Levels of compiled code
//! that hand themselves to the interpreter allocate nothing (`handoff`).

use luna_core::__private::VmJitState;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

mod handoff;
mod os;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use os::executable_bytes;

// the tests measure the whole process, so they take turns
static SERIAL: Mutex<()> = Mutex::new(());

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

/// Allocations made while compiled code runs.
static IN_JIT: AtomicUsize = AtomicUsize::new(0);

fn count_in_jit() {
    if !luna_jit::jit_backend::__jit_tls_ptrs().0.is_null() {
        IN_JIT.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: forwards to `System`, only counting sizes
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc::alloc`'s contract, passed through
        let p = unsafe { System.alloc(layout) };
        count_in_jit();
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `GlobalAlloc::alloc_zeroed`'s contract, passed through
        let p = unsafe { System.alloc_zeroed(layout) };
        count_in_jit();
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
        count_in_jit();
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
    vm.set_trace_hot_threshold(2);
    vm.set_call_hot_threshold(2);
    vm.eval(SRC).expect("eval");
    let chunks = luna_jit::jit::cache_entry_count(&vm);
    (chunks, vm.trace_dispatched_count())
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

/// Runs `SRC` on a fresh Vm of `engine`; returns (traces it installed
/// from the engine, traces dispatched).
fn engine_vm(engine: &luna_jit::Engine) -> (u64, u64) {
    let mut vm = engine.new_vm(luna_jit::LuaVersion::Lua54);
    vm.set_trace_hot_threshold(2);
    vm.set_call_hot_threshold(2);
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
