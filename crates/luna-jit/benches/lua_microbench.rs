//! P10 ceiling — Lua microbench harness. Quantitative baseline for luna's
//! interpreter ceiling, with optional PUC 5.5 and LuaJIT 2.1 comparison
//! when their binaries are on `PATH`.
//!
//! Zero deps (no criterion / divan / etc.): `std::time::Instant` measures
//! wall-clock around `Vm::eval`, samples the median of N runs. PUC and
//! LuaJIT are spawned as subprocesses so their per-iter wall-clock includes
//! process startup — fine for fixed-N comparison since luna's `Vm::new` +
//! library init pays a similar one-time cost up-front; what matters is the
//! relative ordering on iteration-dominated workloads.
//!
//! Run: `cargo bench --bench lua_microbench`

use std::time::{Duration, Instant};

use luna_jit::version::LuaVersion;

struct Bench {
    name: &'static str,
    source: &'static str,
    iters: usize,
}

const BENCHES: &[Bench] = &[
    Bench {
        name: "fib_28",
        source: "local function f(n) \
                   if n < 2 then return n end \
                   return f(n - 1) + f(n - 2) \
                 end \
                 return f(28)",
        iters: 5,
    },
    Bench {
        name: "loop_int_1m",
        source: "local s = 0 \
                 for i = 1, 1000000 do s = s + i end \
                 return s",
        iters: 10,
    },
    Bench {
        name: "table_alloc_10k",
        source: "local t = {} \
                 for i = 1, 10000 do t[i] = {i, i * 2, i * 3} end \
                 return #t",
        iters: 20,
    },
    Bench {
        name: "string_concat_5k",
        source: "local parts = {} \
                 for i = 1, 5000 do parts[i] = tostring(i) end \
                 local s = table.concat(parts, ',') \
                 return #s",
        iters: 20,
    },
    Bench {
        name: "math_loop_100k",
        source: "local s = 0.0 \
                 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end \
                 return s",
        iters: 10,
    },
    Bench {
        name: "closure_alloc_10k",
        source: "local fns = {} \
                 for i = 1, 10000 do \
                   local k = i \
                   fns[i] = function () return k * k end \
                 end \
                 local s = 0 \
                 for i = 1, 10000 do s = s + fns[i]() end \
                 return s",
        iters: 10,
    },
    Bench {
        name: "binary_trees_n10",
        source: "local function make(d) \
                   if d == 0 then return {nil, nil} end \
                   return {make(d - 1), make(d - 1)} \
                 end \
                 local function check(t) \
                   if t[1] == nil then return 1 end \
                   return 1 + check(t[1]) + check(t[2]) \
                 end \
                 local sum = 0 \
                 for i = 1, 16 do sum = sum + check(make(10)) end \
                 return sum",
        iters: 5,
    },
];

/// Run `bench` in a fresh luna Vm `iters` times; return the median wall-clock.
fn measure_luna(bench: &Bench) -> Duration {
    let mut samples: Vec<Duration> = Vec::with_capacity(bench.iters);
    for _ in 0..bench.iters {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
        let start = Instant::now();
        vm.eval(bench.source).expect("benchmark eval");
        samples.push(start.elapsed());
    }
    samples.sort();
    samples[samples.len() / 2]
}

/// Try running the same source through an external Lua interpreter on PATH.
/// `bin` is searched verbatim. Returns None when the binary is absent.
/// Samples include subprocess startup; the comparison is wall-clock and
/// most useful on iteration-dominated benches.
fn measure_external(bin: &str, source: &str, iters: usize) -> Option<Duration> {
    // Probe: ask for `-v` (prints version to stderr and exits 0 on
    // PUC / LuaJIT). If the spawn itself fails the binary isn't on PATH.
    let probe = std::process::Command::new(bin)
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if probe.is_err() {
        return None;
    }
    let mut samples: Vec<Duration> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let out = std::process::Command::new(bin)
            .arg("-e")
            .arg(source)
            .output();
        if out.is_err() {
            return None;
        }
        samples.push(start.elapsed());
    }
    samples.sort();
    Some(samples[samples.len() / 2])
}

fn fmt_us(d: Duration) -> String {
    let us = d.as_micros();
    if us < 10_000 {
        format!("{us}us")
    } else {
        format!("{}ms", us / 1000)
    }
}

fn ratio(luna: Duration, other: Duration) -> String {
    let r = luna.as_secs_f64() / other.as_secs_f64();
    format!("{r:>5.2}x")
}

fn main() {
    println!("luna microbench (median of N runs)");
    println!(
        "{:<22} {:>5} {:>10} {:>10} {:>10} {:>8} {:>8}",
        "bench", "iters", "luna", "puc5.5", "luajit", "vs.puc", "vs.ljit"
    );
    println!("{:-<78}", "");

    // Detect external interpreters once (per-bench probing also tolerates
    // a missing binary, but spelling them in one place keeps the report
    // header tidy).
    let puc_bins = ["lua5.5", "lua-5.5", "lua"];
    let ljit_bins = ["luajit", "luajit-2.1", "luajit2"];

    for b in BENCHES {
        let luna_d = measure_luna(b);
        let puc_d = puc_bins
            .iter()
            .find_map(|bin| measure_external(bin, b.source, b.iters));
        let ljit_d = ljit_bins
            .iter()
            .find_map(|bin| measure_external(bin, b.source, b.iters));
        let puc_s = puc_d.map(fmt_us).unwrap_or_else(|| "-".into());
        let ljit_s = ljit_d.map(fmt_us).unwrap_or_else(|| "-".into());
        let puc_r = puc_d
            .map(|d| ratio(luna_d, d))
            .unwrap_or_else(|| "-".into());
        let ljit_r = ljit_d
            .map(|d| ratio(luna_d, d))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<22} {:>5} {:>10} {:>10} {:>10} {:>8} {:>8}",
            b.name,
            b.iters,
            fmt_us(luna_d),
            puc_s,
            ljit_s,
            puc_r,
            ljit_r,
        );
    }
    println!();
    println!("notes:");
    println!("  vs.puc / vs.ljit = luna_time / other_time (>1 = luna slower)");
    println!("  PUC + LuaJIT timings include subprocess startup");
    println!("  set $PATH so `lua5.5` / `luajit` resolve for comparison");
}
