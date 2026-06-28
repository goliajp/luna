//! v2.1 Phase 1K.H bench — fib(28) µs/cell measurement (single eval per cell).
//!
//! Pattern matches `bench_a4_prime_token_bucket.rs`: 5 warmup + 30 timed
//! reruns, prints µs per cell + summary line. Selects backend via
//! `LUNA_JIT_BACKEND` env var.

use std::hint::black_box;
use std::time::Instant;

use luna_jit::new_minimal_with_jit;
use luna_jit::version::LuaVersion;

const RUNS: usize = 30;

const SRC: &str = r#"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
"#;

fn main() {
    for _ in 0..5 {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
        let _ = black_box(vm.eval(SRC).expect("eval"));
    }

    println!("# fib(28) µs/cell (one cell = one fib(28) eval)");
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
        vm.open_base();
        vm.open_math();
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
