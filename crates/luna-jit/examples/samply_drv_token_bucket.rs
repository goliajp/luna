//! v2.1 Phase 1K.I samply driver — token_bucket_1k steady-state ~3-5 s.
//!
//! Pure research instrumentation. Mirrors `bench_a4_prime_token_bucket.rs`
//! Lua source. Selects backend via `LUNA_JIT_BACKEND`. ITERS chosen so
//! 1 cell × ITERS spans ~3-5s on both backends so samply gets >15k
//! samples per backend at default --rate.

use std::hint::black_box;

use luna_jit::new_minimal_with_jit;
use luna_jit::version::LuaVersion;

const SRC: &str = r#"
    local bucket = { tokens = 1000, last = 0, rate = 100 }
    local now = 1
    local refilled = 0
    for i = 1, 1000 do
        local elapsed = now - bucket.last
        local refill = elapsed * bucket.rate
        if refill > 0 then
            bucket.tokens = math.min(1000, bucket.tokens + refill)
            bucket.last = now
            refilled = refilled + 1
        end
        if bucket.tokens >= 1 then
            bucket.tokens = bucket.tokens - 1
        end
        now = now + 1
    end
    return bucket.tokens, refilled
"#;

fn iters() -> usize {
    std::env::var("SAMPLY_DRV_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15000) // cell ≈ 170-230 µs, 15k × 200 µs ≈ 3 s
}

fn main() {
    for _ in 0..5 {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        let _ = black_box(vm.eval(SRC).expect("warmup"));
    }

    let n = iters();
    for _ in 0..n {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        let _ = black_box(vm.eval(SRC).expect("eval"));
    }
}
