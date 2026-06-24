//! v1.3 P2A — quick perf gauge: token_bucket with trace JIT on vs off.
//!
//! NOT a formal bench (criterion handles statistical sampling +
//! variance reporting). This is a directional smoke check: median
//! over N=51 runs per mode, after a 5-run warmup. macOS variance
//! is wide enough that small (sub-10%) deltas should NOT be trusted
//! here; the formal Linux taskset bench is the gate per the audit's
//! methodology §2 Phase A.

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

const WARMUP: usize = 5;
const N: usize = 51;

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

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    // Warmup
    for _ in 0..WARMUP {
        let _ = time_one(false);
        let _ = time_one(true);
    }
    let interp: Vec<_> = (0..N).map(|_| time_one(false)).collect();
    let trace: Vec<_> = (0..N).map(|_| time_one(true)).collect();
    let m_interp = median(interp);
    let m_trace = median(trace);

    println!("# v1.3 P2A — token_bucket trace JIT engagement perf gauge");
    println!("# N={} per mode, median reported; WARMUP={}", N, WARMUP);
    println!();
    println!("interp (trace_enabled=false):  median = {:?}", m_interp);
    println!("trace  (trace_enabled=true):   median = {:?}", m_trace);
    let ratio = m_interp.as_nanos() as f64 / m_trace.as_nanos() as f64;
    println!("trace speedup vs interp:       {:.3}× (>1 = faster)", ratio);
    println!();
    println!("# macOS variance is wide; formal Linux taskset bench is the gate.");
}
