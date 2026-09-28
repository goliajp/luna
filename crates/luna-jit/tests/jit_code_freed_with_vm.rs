//! The machine code the method and trace JIT compile for a `Vm` is freed
//! when that `Vm` drops. Code memory comes from the global allocator, so
//! a counting allocator sees it: creating and dropping many `Vm`s that
//! each compile functions and traces must not grow the live bytes.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

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
