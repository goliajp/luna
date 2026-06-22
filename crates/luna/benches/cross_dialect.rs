//! Cross-dialect microbench — compares luna against the canonical reference
//! interpreter at each Lua dialect (5.1 → 5.5) plus LuaJIT 2.1 as the
//! "best in community" baseline for the 5.1 lineage.
//!
//! Run: `cargo bench --bench cross_dialect`
//!
//! External binaries probed (configurable via PATH):
//!   lua-5.1 / lua-5.2 / lua-5.3 / lua-5.4 / lua-5.5   — PUC official
//!   luajit                                            — LuaJIT 2.1 (5.1 base)
//!
//! Wall-clock includes subprocess startup for the PUC / LuaJIT cells —
//! luna's `Vm::new` + lib init pays a similar one-time cost up-front, so
//! the comparison stays fair on iteration-dominated workloads.

use std::time::{Duration, Instant};

use luna::version::LuaVersion;
use luna::vm::Vm;

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

fn measure_luna(version: LuaVersion, bench: &Bench) -> Duration {
    let mut samples: Vec<Duration> = Vec::with_capacity(bench.iters);
    for _ in 0..bench.iters {
        let mut vm = luna::new_with_jit(version);
        let start = Instant::now();
        vm.eval(bench.source).expect("benchmark eval");
        samples.push(start.elapsed());
    }
    samples.sort();
    samples[samples.len() / 2]
}

fn measure_external(bin: &str, source: &str, iters: usize) -> Option<Duration> {
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

struct Dialect {
    version: LuaVersion,
    label: &'static str,
    /// Reference interpreter binaries to probe in order; first one on
    /// PATH wins.
    refs: &'static [&'static str],
}

const DIALECTS: &[Dialect] = &[
    Dialect { version: LuaVersion::Lua51, label: "Lua 5.1", refs: &["lua-5.1", "lua5.1"] },
    Dialect { version: LuaVersion::Lua52, label: "Lua 5.2", refs: &["lua-5.2", "lua5.2"] },
    Dialect { version: LuaVersion::Lua53, label: "Lua 5.3", refs: &["lua-5.3", "lua5.3"] },
    Dialect { version: LuaVersion::Lua54, label: "Lua 5.4", refs: &["lua-5.4", "lua5.4"] },
    Dialect { version: LuaVersion::Lua55, label: "Lua 5.5", refs: &["lua-5.5", "lua5.5", "lua"] },
];

fn main() {
    println!("luna cross-dialect microbench (median of N runs per cell)");
    println!();

    for dialect in DIALECTS {
        let puc_bin: Option<&str> = dialect.refs.iter().find_map(|b| {
            let probe = std::process::Command::new(b)
                .arg("-v")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if probe.is_ok() { Some(*b) } else { None }
        });
        let puc_label = puc_bin.unwrap_or("(not on PATH)");
        println!("=== {} ===", dialect.label);
        println!(
            "{:<22} {:>5} {:>10} {:>12} {:>8}",
            "bench", "iters", "luna", puc_label, "vs.puc"
        );
        println!("{:-<64}", "");
        for b in BENCHES {
            let luna_d = measure_luna(dialect.version, b);
            let puc_d = puc_bin.and_then(|bin| measure_external(bin, b.source, b.iters));
            let puc_s = puc_d.map(fmt_us).unwrap_or_else(|| "-".into());
            let puc_r = puc_d.map(|d| ratio(luna_d, d)).unwrap_or_else(|| "-".into());
            println!(
                "{:<22} {:>5} {:>10} {:>12} {:>8}",
                b.name,
                b.iters,
                fmt_us(luna_d),
                puc_s,
                puc_r,
            );
        }
        println!();
    }

    // LuaJIT 2.1 is the production-favourite community runtime for the
    // 5.1 lineage (5.2 extensions opt-in). Show it side-by-side with luna
    // configured as 5.1 since that's what LuaJIT implements.
    let ljit_bin: Option<&str> = ["luajit", "luajit-2.1", "luajit2"]
        .iter()
        .find_map(|b| {
            let probe = std::process::Command::new(b)
                .arg("-v")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if probe.is_ok() { Some(*b) } else { None }
        });
    if let Some(bin) = ljit_bin {
        println!("=== LuaJIT 2.1 (vs luna @ Lua 5.1) ===");
        println!(
            "{:<22} {:>5} {:>10} {:>12} {:>8}",
            "bench", "iters", "luna5.1", bin, "vs.ljit"
        );
        println!("{:-<64}", "");
        for b in BENCHES {
            let luna_d = measure_luna(LuaVersion::Lua51, b);
            let ljit_d = measure_external(bin, b.source, b.iters);
            let ljit_s = ljit_d.map(fmt_us).unwrap_or_else(|| "-".into());
            let ljit_r = ljit_d.map(|d| ratio(luna_d, d)).unwrap_or_else(|| "-".into());
            println!(
                "{:<22} {:>5} {:>10} {:>12} {:>8}",
                b.name,
                b.iters,
                fmt_us(luna_d),
                ljit_s,
                ljit_r,
            );
        }
        println!();
    } else {
        println!("(luajit not on PATH — skipping LuaJIT comparison)");
    }

    println!("notes:");
    println!("  - vs.X = luna_time / X_time (>1 = luna slower than that reference)");
    println!("  - PUC + LuaJIT timings include subprocess startup");
    println!("  - luna is interpreter-only; LuaJIT JITs hot loops — expect a large gap");
}
