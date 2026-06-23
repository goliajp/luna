//! Run-only microbench — measures JIT execution time after the Proto
//! is loaded and the JIT cache is warm.
//!
//! Companion to `cross_dialect.rs` which times `Vm::new + vm.load +
//! call_value` per iteration and is dominated by parse + Vm setup
//! overhead (~99% for the table_alloc / fib / loop cells). That
//! comparison is fair against PUC's subprocess `lua -e <src>`
//! invocation (also parse-dominated) but hides any per-iter JIT
//! improvement under parser noise.
//!
//! This bench:
//!   1. Loads each source ONCE on a single Vm
//!   2. Calls the Proto once to populate the JIT cache
//!   3. Times N back-to-back `call_value` invocations on the warmed-up Proto
//!   4. Reports the median of N timing samples
//!
//! PUC + LuaJIT comparison: we invoke each external interpreter with
//! a wrapper script that pre-loads the body once via `load(src)()`
//! to warm the closure, then calls the wrapper N times in a tight
//! loop and prints the per-call wall-clock via `os.clock()`. This
//! matches the luna run-only methodology — the subprocess fork +
//! parser overhead is amortised across N inner calls.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Bench {
    name: &'static str,
    source: &'static str,
    /// Number of back-to-back call_value invocations per sample.
    inner_calls: usize,
    /// Number of samples to take. Median is reported.
    samples: usize,
}

const BENCHES: &[Bench] = &[
    Bench {
        name: "table_alloc_10k",
        source: "local t = {} for i = 1, 10000 do t[i] = i end return #t",
        inner_calls: 100,
        samples: 11,
    },
    Bench {
        name: "math_loop_100k",
        source: "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s",
        inner_calls: 20,
        samples: 11,
    },
    Bench {
        name: "loop_int_1m",
        source: "local s = 0 for i = 1, 1000000 do s = s + i end return s",
        inner_calls: 20,
        samples: 11,
    },
    Bench {
        name: "fib_28",
        source: "local function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end return fib(28)",
        inner_calls: 20,
        samples: 11,
    },
    Bench {
        name: "binary_trees_n10",
        source: "local function make(d) \
                   if d == 0 then return {1,1} \
                   else return {make(d-1), make(d-1)} end \
                 end \
                 local function check(t) \
                   if t[1] == 1 then return 1 end \
                   return 1 + check(t[1]) + check(t[2]) \
                 end \
                 local sum = 0 \
                 for i = 1, 16 do sum = sum + check(make(10)) end \
                 return sum",
        inner_calls: 5,
        samples: 11,
    },
];

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "Lua 5.1"),
    (LuaVersion::Lua52, "Lua 5.2"),
    (LuaVersion::Lua53, "Lua 5.3"),
    (LuaVersion::Lua54, "Lua 5.4"),
    (LuaVersion::Lua55, "Lua 5.5"),
];

fn measure(version: LuaVersion, bench: &Bench) -> Duration {
    let mut vm = luna_jit::new_with_jit(version);
    let cl = vm.load(bench.source.as_bytes(), b"=t").expect("compile");
    // Warmup: first invocation walks `populate_jit_cache` (which
    // might compile or hit the thread-local cache from a prior
    // bench cell). Subsequent calls are purely JIT-entry +
    // helpers.
    let _ = vm.call_value(Value::Closure(cl), &[]).expect("warmup");

    let mut samples: Vec<Duration> = Vec::with_capacity(bench.samples);
    for _ in 0..bench.samples {
        let start = Instant::now();
        for _ in 0..bench.inner_calls {
            let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        }
        let elapsed = start.elapsed();
        samples.push(elapsed / bench.inner_calls as u32);
    }
    samples.sort();
    samples[samples.len() / 2]
}

fn fmt_ns(d: Duration) -> String {
    let ns = d.as_nanos();
    if ns < 10_000 {
        format!("{ns}ns")
    } else if ns < 10_000_000 {
        format!("{}us", ns / 1000)
    } else {
        format!("{}ms", ns / 1_000_000)
    }
}

fn fmt_ratio(luna: Duration, other: Duration) -> String {
    if other.is_zero() {
        return "-".into();
    }
    let r = luna.as_secs_f64() / other.as_secs_f64();
    format!("{r:>5.2}x")
}

fn probe_bin(bin: &str) -> bool {
    Command::new(bin)
        .arg("-v")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Per-call median measured inside the subprocess. The wrapper loads
/// the bench source once, calls it once to populate any per-fn
/// caches the runtime maintains, then times `inner_calls` back-to-
/// back invocations via `os.clock()`. Multiple `samples` are taken
/// and the median is printed by the subprocess and parsed back here.
fn measure_external(bin: &str, bench: &Bench) -> Option<Duration> {
    let wrapper = format!(
        "local fn = assert(loadstring or load)([==[{src}]==]) \
         local results = {{}} \
         fn() \
         for s = 1, {samples} do \
           local t0 = os.clock() \
           for i = 1, {inner} do fn() end \
           results[s] = (os.clock() - t0) / {inner} \
         end \
         table.sort(results) \
         io.write(string.format('%.9f', results[math.ceil(#results / 2)]))",
        src = bench.source,
        samples = bench.samples,
        inner = bench.inner_calls,
    );
    let out = Command::new(bin).arg("-e").arg(&wrapper).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let secs: f64 = s.trim().parse().ok()?;
    Some(Duration::from_secs_f64(secs))
}

const PUC_BINS: &[(&str, &[&str])] = &[
    ("Lua 5.1", &["lua-5.1", "lua5.1"]),
    ("Lua 5.2", &["lua-5.2", "lua5.2"]),
    ("Lua 5.3", &["lua-5.3", "lua5.3"]),
    ("Lua 5.4", &["lua-5.4", "lua5.4"]),
    ("Lua 5.5", &["lua-5.5", "lua5.5"]),
];
const LUAJIT_BINS: &[&str] = &["luajit-2.1", "luajit"];

fn first_present(candidates: &[&str]) -> Option<&'static str> {
    for &c in candidates {
        if probe_bin(c) {
            // Leak the &str so it lives long enough for the bench. Cheap
            // (one alloc per dialect) and avoids threading a `String`
            // through. Could also use a different lifetime strategy.
            let leaked: &'static str = Box::leak(c.to_string().into_boxed_str());
            return Some(leaked);
        }
    }
    None
}

fn main() {
    println!("luna run-only microbench — per-call median, JIT-warmed Proto cached on a single Vm");
    println!(
        "External columns measure the same per-call shape via `os.clock()` inside the subprocess."
    );
    println!();
    let luajit_bin = first_present(LUAJIT_BINS);
    for (idx, (version, label)) in DIALECTS.iter().enumerate() {
        let puc_bin = first_present(PUC_BINS[idx].1);
        let puc_label = puc_bin.unwrap_or("(no PATH)");
        println!("=== {label} ===");
        println!(
            "{:<22} {:>10} {:>12} {:>8} {:>10} {:>8}",
            "bench", "luna", puc_label, "vs.puc", "luajit", "vs.ljit"
        );
        println!("{:-<78}", "");
        for b in BENCHES {
            let luna_d = measure(*version, b);
            let puc_d = puc_bin.and_then(|p| measure_external(p, b));
            let ljit_d = luajit_bin.and_then(|p| measure_external(p, b));
            let puc_s = puc_d.map(fmt_ns).unwrap_or_else(|| "-".into());
            let puc_r = puc_d
                .map(|d| fmt_ratio(luna_d, d))
                .unwrap_or_else(|| "-".into());
            let ljit_s = ljit_d.map(fmt_ns).unwrap_or_else(|| "-".into());
            let ljit_r = ljit_d
                .map(|d| fmt_ratio(luna_d, d))
                .unwrap_or_else(|| "-".into());
            println!(
                "{:<22} {:>10} {:>12} {:>8} {:>10} {:>8}",
                b.name,
                fmt_ns(luna_d),
                puc_s,
                puc_r,
                ljit_s,
                ljit_r,
            );
        }
        println!();
    }
}
