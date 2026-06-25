//! v2.0 PI Phase 1 step 0 — detailed token_bucket_1k baseline reporter.
//!
//! Reports min/p50/p95/max for both luna_interp (trace disabled) and
//! luna_trace (trace JIT enabled), so the numbers line up directly
//! with the LuaJIT `-joff` / LuaJIT JIT-on / PUC 5.5 data captured
//! via os.clock() in `.dev/baselines/perf-2026-06-25/token_bucket_1k_timed.lua`.
//!
//! NOT a formal bench. Per perf-decomposition methodology §8 the formal
//! gate is Linux taskset criterion; this is a directional snapshot.
//!
//! Usage: `cargo run --release --example perf_token_bucket_detailed`

use std::time::{Duration, Instant};

use luna_jit::version::LuaVersion;

const TOKEN_BUCKET_SRC: &str = r#"
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

const WARMUP: usize = 10;
const N: usize = 101;

fn time_one(trace_jit: bool) -> Duration {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_trace_jit_enabled(trace_jit);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    let start = Instant::now();
    vm.eval(TOKEN_BUCKET_SRC).expect("script");
    start.elapsed()
}

fn pct(v: &[Duration], p: f64) -> Duration {
    let idx = ((v.len() - 1) as f64 * p) as usize;
    v[idx]
}

fn report(label: &str, mut v: Vec<Duration>) {
    v.sort();
    let mn = v[0];
    let p50 = pct(&v, 0.5);
    let p95 = pct(&v, 0.95);
    let mx = v[v.len() - 1];
    println!(
        "{:<32} min={:>9.3?}  p50={:>9.3?}  p95={:>9.3?}  max={:>9.3?}",
        label, mn, p50, p95, mx
    );
}

fn main() {
    for _ in 0..WARMUP {
        let _ = time_one(false);
        let _ = time_one(true);
    }
    let interp: Vec<_> = (0..N).map(|_| time_one(false)).collect();
    let trace: Vec<_> = (0..N).map(|_| time_one(true)).collect();

    println!("# v2.0 PI Phase 1 step 0 — token_bucket_1k luna baselines");
    println!("# N={} per mode, WARMUP={}", N, WARMUP);
    println!("# 1 cell = full eval of the 1000-iter token_bucket loop body");
    println!();
    report("luna_interp (trace_jit=false)", interp);
    report("luna_trace  (trace_jit=true)", trace);
}
