//! Mini-bench for v2.1 A4' attack — token_bucket_1k µs/cell measurement.
//!
//! Runs the same source as `benches/redis_lua_shape.rs` token_bucket_1k
//! cell (1000 outer-loop iterations per cell) `RUNS` times and prints
//! per-run elapsed in µs. Designed for hand pre-vs-post A/B compare:
//!   - build + run twice on the post-A4' tree → series A
//!   - revert the A4' edit (or stash) + build + run → series B
//!   - compare medians
//!
//! Output is intentionally minimal (one number per line for paste-into
//! a Python median) — the first line is a header.

use std::hint::black_box;
use std::time::Instant;

use luna_jit::new_minimal_with_jit;
use luna_jit::version::LuaVersion;

const RUNS: usize = 30;

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

fn main() {
    // Warm up — 5 untimed iterations so the JIT recorder + cranelift
    // caches stabilise.
    for _ in 0..5 {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        let _ = black_box(vm.eval(SRC).expect("eval"));
    }

    println!("# token_bucket_1k µs/cell (one cell = 1000 inner iters)");
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        let t0 = Instant::now();
        let _ = black_box(vm.eval(SRC).expect("eval"));
        let elapsed = t0.elapsed();
        let us = elapsed.as_secs_f64() * 1e6;
        samples.push(us);
        println!("{us:.3}");
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = samples.len();
    let median = if n % 2 == 0 {
        (samples[n / 2 - 1] + samples[n / 2]) / 2.0
    } else {
        samples[n / 2]
    };
    let mean = samples.iter().sum::<f64>() / n as f64;
    let p25 = samples[n / 4];
    let p75 = samples[3 * n / 4];
    let min = samples[0];
    let max = samples[n - 1];
    eprintln!(
        "# n={n} min={min:.3} p25={p25:.3} median={median:.3} mean={mean:.3} p75={p75:.3} max={max:.3}"
    );
}
