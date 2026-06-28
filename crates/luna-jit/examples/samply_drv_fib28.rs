//! v2.1 Phase 1K.I samply driver — fib(28) steady-state, ~3-5 s of work.
//!
//! Pure research instrumentation. No timing output, no assertions on µs.
//! Mirrors `bench_1k_h_fib28.rs` Lua source. Selects backend via
//! `LUNA_JIT_BACKEND` env var. ITERS chosen so 1 cell × ITERS = ~3-5s
//! on both Cranelift (1.1 ms/cell) and LLVM (1.8 ms/cell) so samply
//! gets ~3000-5000 ms × 5000 Hz = 15k-25k samples per backend.

use std::hint::black_box;

use luna_jit::new_minimal_with_jit;
use luna_jit::version::LuaVersion;

const SRC: &str = r#"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
"#;

// Override via SAMPLY_DRV_ITERS env var if needed.
fn iters() -> usize {
    std::env::var("SAMPLY_DRV_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2500)
}

fn main() {
    // Warmup: fresh Vm + 5 cells so JIT compile / dispatcher warm.
    for _ in 0..5 {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        let _ = black_box(vm.eval(SRC).expect("warmup"));
    }

    let n = iters();
    for _ in 0..n {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        let _ = black_box(vm.eval(SRC).expect("eval"));
    }
}
