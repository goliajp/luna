//! v2.13 Track WUC Prong A — poison-on-free allocator stress for
//! UAF-C (Windows gc.lua weak-table STATUS_ACCESS_VIOLATION).
//!
//! Hypothesis (v2.8, `.dev/known-bugs/windows-gc-weak-table-uaf-c.md`):
//! the UAF exists on all platforms; the Windows Heap Manager's
//! freed-memory fill pattern makes the weak-table sweep's stale
//! read *visible* as an AV, while glibc/jemalloc/Apple-malloc leave
//! freed bits looking benign. luna's GC frees every collected
//! object through `Box::from_raw` → the global allocator
//! (`runtime/heap.rs::free_obj`), so a `#[global_allocator]` that
//! fills freed blocks with 0xDD (the MSVC debug-heap dead-memory
//! byte) reproduces the Windows visibility condition on any host.
//!
//! Run (slow, diagnostic — gated behind `--ignored`):
//!     cargo test --release -p luna-core --test gc_stress_poison \
//!         -- --ignored --nocapture
//!     LUNA_POISON_STRESS_N=25 cargo test ... (default 5 iterations)
//!
//! A SIGSEGV / garbage panic / wrong assert inside this test =
//! UAF-C reproduced locally → debug under ASAN from here.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::mpsc;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

struct PoisonOnFree;

// SAFETY: pure delegation to `System`; the 0xDD fill happens while
// the block is still owned (before System::dealloc), which is legal.
unsafe impl GlobalAlloc for PoisonOnFree {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            std::ptr::write_bytes(ptr, 0xDD, layout.size());
            System.dealloc(ptr, layout)
        }
    }
}

#[global_allocator]
static POISON: PoisonOnFree = PoisonOnFree;

fn run_one(file: &str) -> Result<(), String> {
    let raw = std::fs::read(file).map_err(|e| format!("read {file}: {e}"))?;
    let stripped = luna_core::frontend::lexer::Lexer::strip_shebang_bom(&raw).to_vec();
    let chunkname = format!("@{file}");
    let label = file.to_string();
    // Same 16 MiB-stack worker thread shape as official_run.rs —
    // gc.lua recursion depth needs it.
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = Vm::new(LuaVersion::Lua55);
            vm.set_global("_U", Value::Bool(true)).unwrap();
            let r: Result<(), String> = match vm.load(&stripped, chunkname.as_bytes()) {
                Ok(cl) => match vm.call_value(Value::Closure(cl), &[]) {
                    Ok(_) => Ok(()),
                    Err(e) => Err(format!("runtime: {:.300}", vm.error_text(&e))),
                },
                Err(e) => Err(format!("compile: {e}")),
            };
            let _ = tx.send(r);
        })
        .map_err(|e| format!("spawn {label}: {e}"))?
        .join()
        .map_err(|_| format!("{file}: worker thread panicked (UAF-C candidate!)"))?;
    rx.recv().map_err(|e| format!("recv: {e}"))?
}

#[test]
#[ignore = "UAF-C diagnostic stress (slow); run with --ignored"]
fn gc_lua_family_under_poison_allocator() {
    let n: usize = std::env::var("LUNA_POISON_STRESS_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/official/lua-5.5.0-tests");
    std::env::set_current_dir(&suite).expect("suite dir");
    for iter in 1..=n {
        for file in ["gc.lua", "gengc.lua", "tracegc.lua"] {
            eprintln!("[poison-stress] iter {iter}/{n} {file}");
            if let Err(e) = run_one(file) {
                panic!("[poison-stress] iter {iter} {file} FAILED: {e}");
            }
        }
    }
    eprintln!("[poison-stress] {n} iterations clean — no UAF surfaced under 0xDD poison");
}
