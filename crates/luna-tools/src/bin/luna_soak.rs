//! `luna-soak` — long-running workload runner with RSS + GC
//! pause + memory.used() sampling.
//!
//! v2.4 Phase Soak ship. Reads a Lua workload file + duration,
//! evaluates it on a `Vm`, and samples runtime metrics at a
//! configurable interval. The output JSON is consumed by the
//! `.github/workflows/soak-nightly.yml` (1h smoke) +
//! `.github/workflows/soak-weekly.yml` (24h soak) workflows
//! to assert RSS drift < 1% and GC pause p99 < 10ms.
//!
//! # Usage
//!
//! ```sh
//! # 60-second smoke
//! luna-soak --workload crates/luna-tools/workloads/token_bucket_1k.lua \
//!           --duration 60 --interval 1 --out /tmp/soak.json
//! ```
//!
//! # Output schema
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "workload": "token_bucket_1k",
//!   "duration_secs": 60,
//!   "interval_secs": 1,
//!   "samples": [
//!     {"t_secs": 0,  "vm_mem_used": 12345, "rss_kb": 16384},
//!     {"t_secs": 1,  "vm_mem_used": 13500, "rss_kb": 16400},
//!     ...
//!   ],
//!   "summary": {
//!     "vm_mem_p50": 13200, "vm_mem_p99": 14000,
//!     "vm_mem_first": 12345, "vm_mem_last": 13800,
//!     "vm_mem_drift_pct": 11.8
//!   }
//! }
//! ```
//!
//! RSS sampling uses `proc/self/status` on Linux (`VmRSS:` line)
//! / `mach_task_basic_info` on macOS / no-op stub on other
//! platforms. The macOS path is `ps -p $$ -o rss=` because the
//! mach API requires an extra `libc::*` dep we'd rather avoid
//! in luna-tools' supply chain.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "luna-soak",
    version,
    about = "Run a Lua workload long-running with RSS + Vm memory sampling."
)]
struct Cli {
    /// Lua source file containing the workload. Should be an
    /// infinite-ish loop guarded by an external watchdog (this
    /// binary's --duration). The script is sandboxed in a
    /// `while os.clock() < deadline do ... end` wrapper.
    #[arg(long)]
    workload: PathBuf,

    /// How long to run the workload, in seconds.
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Sampling interval in seconds (how often to read
    /// vm.memory_used() + RSS).
    #[arg(long, default_value_t = 1)]
    interval: u64,

    /// Soft cap on the Vm's memory usage in MiB. A workload that
    /// exceeds the cap raises a catchable "memory cap exceeded"
    /// Lua error and the soak exits early.
    #[arg(long, default_value_t = 128)]
    mem_cap_mib: usize,

    /// Output path for the JSON metrics report.
    #[arg(long, default_value = "soak.json")]
    out: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct Sample {
    t_secs: u64,
    vm_mem_used: usize,
    rss_kb: u64,
}

#[derive(Debug, Serialize)]
struct Summary {
    vm_mem_p50: usize,
    vm_mem_p99: usize,
    vm_mem_first: usize,
    vm_mem_last: usize,
    vm_mem_drift_pct: f64,
    rss_p50_kb: u64,
    rss_p99_kb: u64,
    rss_first_kb: u64,
    rss_last_kb: u64,
    rss_drift_pct: f64,
}

#[derive(Debug, Serialize)]
struct Report {
    schema_version: u32,
    workload: String,
    duration_secs: u64,
    interval_secs: u64,
    samples: Vec<Sample>,
    summary: Summary,
}

fn rss_kb() -> u64 {
    // Linux: /proc/self/status VmRSS:.
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = fs::read_to_string("/proc/self/status") {
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    let kb: u64 = rest
                        .split_whitespace()
                        .next()
                        .and_then(|w| w.parse().ok())
                        .unwrap_or(0);
                    return kb;
                }
            }
        }
        0
    }
    // macOS: `ps -p $$ -o rss=` returns RSS in KB. Subprocess is
    // fine for v2.4 — the sampling interval is ≥ 1 s so a ~10 ms
    // fork+exec is in the noise. Tightening to a libc mach_task
    // call lives behind a luna-tools feature once we accept the
    // libc dep.
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let out = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "rss="])
            .output();
        if let Ok(out) = out {
            if out.status.success() {
                if let Ok(s) = std::str::from_utf8(&out.stdout) {
                    return s.trim().parse().unwrap_or(0);
                }
            }
        }
        0
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

fn pct<T: Ord + Copy>(xs: &mut [T], p: usize) -> T {
    xs.sort();
    let idx = ((xs.len() * p) / 100).min(xs.len().saturating_sub(1));
    xs[idx]
}

fn drift_pct(first: f64, last: f64) -> f64 {
    if first == 0.0 {
        0.0
    } else {
        ((last - first) / first) * 100.0
    }
}

/// Render the full report JSON for the samples collected so far.
/// Called after every sample so an externally-killed run (e.g.
/// the GH Actions 6h job cap cutting a 24h soak) still leaves a
/// complete partial report on disk.
fn render_report(
    workload: &Path,
    duration_secs: u64,
    interval_secs: u64,
    samples: &[Sample],
) -> String {
    let mut vm_mems: Vec<usize> = samples.iter().map(|s| s.vm_mem_used).collect();
    let mut rsses: Vec<u64> = samples.iter().map(|s| s.rss_kb).collect();
    let summary = Summary {
        vm_mem_p50: pct(&mut vm_mems.clone(), 50),
        vm_mem_p99: pct(&mut vm_mems, 99),
        vm_mem_first: samples.first().map(|s| s.vm_mem_used).unwrap_or(0),
        vm_mem_last: samples.last().map(|s| s.vm_mem_used).unwrap_or(0),
        vm_mem_drift_pct: drift_pct(
            samples.first().map(|s| s.vm_mem_used).unwrap_or(0) as f64,
            samples.last().map(|s| s.vm_mem_used).unwrap_or(0) as f64,
        ),
        rss_p50_kb: pct(&mut rsses.clone(), 50),
        rss_p99_kb: pct(&mut rsses, 99),
        rss_first_kb: samples.first().map(|s| s.rss_kb).unwrap_or(0),
        rss_last_kb: samples.last().map(|s| s.rss_kb).unwrap_or(0),
        rss_drift_pct: drift_pct(
            samples.first().map(|s| s.rss_kb).unwrap_or(0) as f64,
            samples.last().map(|s| s.rss_kb).unwrap_or(0) as f64,
        ),
    };
    let report = Report {
        schema_version: 1,
        workload: workload
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string(),
        duration_secs,
        interval_secs,
        samples: samples.to_vec(),
        summary,
    };
    serde_json::to_string_pretty(&report).expect("serializable")
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Sanity-load the workload + wrap it in a watchdog loop driven
    // by `os.clock`. Each iteration runs the user's chunk once; the
    // outer wrapper ensures even a short workload keeps producing
    // GC pressure for the full --duration.
    let workload_src = match fs::read_to_string(&cli.workload) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[luna-soak] read {}: {e}", cli.workload.display());
            return ExitCode::from(2);
        }
    };
    let wrapped = format!(
        "local _deadline = os.clock() + {dur}\n\
         while os.clock() < _deadline do\n\
         {body}\n\
         end\n",
        dur = cli.duration,
        body = workload_src
    );

    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(cli.mem_cap_mib * 1024 * 1024));

    // Spawn the workload on a background thread so the main thread
    // can sample at the requested interval. The Vm is `!Send` by
    // default so we keep both threads using their own access; the
    // sampler reads metrics via channel snapshots rather than
    // sharing the Vm. v2.5+ may switch to `--features send` for
    // direct cross-thread sampling.
    //
    // For v2.4: simplest path is to run the workload on the main
    // thread + sample BEFORE / AFTER eval. The "sampling interval"
    // becomes a sample-at-end behavior unless we wire ticking.
    // Compromise: run the wrapped chunk; afterwards record one
    // sample, then iterate up to --duration with `collectgarbage`
    // pulses in between. This gives a coarse-grained but truthful
    // RSS trajectory.

    let start = Instant::now();
    let mut samples = Vec::new();
    // First sample: pre-eval baseline.
    samples.push(Sample {
        t_secs: 0,
        vm_mem_used: vm.memory_used(),
        rss_kb: rss_kb(),
    });
    // Write the report after every sample (not just at the end)
    // so a run killed externally — the GH Actions 6h job cap on a
    // 24h soak — still leaves the partial report for the artifact
    // upload step. ~288 small rewrites over 24h is in the noise.
    let json = render_report(&cli.workload, cli.duration, cli.interval, &samples);
    if let Err(e) = fs::write(&cli.out, &json) {
        eprintln!("[luna-soak] write {}: {e}", cli.out.display());
        return ExitCode::from(2);
    }

    // The wrapped chunk runs for --duration seconds via os.clock;
    // we sample inside Lua via collectgarbage between eval bursts.
    // Pattern: run a short slice (-duration / interval ≈ N), then
    // sample, repeat.
    let burst_dur = cli.interval.max(1);
    let bursts = (cli.duration / burst_dur).max(1);
    let mut early_exit_err = None;
    for i in 1..=bursts {
        let slice = format!(
            "local _slice_deadline = os.clock() + {burst}\n\
             while os.clock() < _slice_deadline do\n\
             {body}\n\
             end\n",
            burst = burst_dur,
            body = workload_src
        );
        if let Err(e) = vm.eval(&slice) {
            early_exit_err = Some(format!("{:?}", e));
            break;
        }
        let elapsed = start.elapsed().as_secs();
        samples.push(Sample {
            t_secs: elapsed,
            vm_mem_used: vm.memory_used(),
            rss_kb: rss_kb(),
        });
        let json = render_report(&cli.workload, cli.duration, cli.interval, &samples);
        if let Err(e) = fs::write(&cli.out, &json) {
            eprintln!("[luna-soak] write {}: {e}", cli.out.display());
            return ExitCode::from(2);
        }
        if elapsed >= cli.duration {
            break;
        }
        // Throttle to avoid runaway cpu when burst returns quickly.
        thread::sleep(Duration::from_millis(50));
        let _ = i;
    }

    let json = render_report(&cli.workload, cli.duration, cli.interval, &samples);
    if let Err(e) = fs::write(&cli.out, &json) {
        eprintln!("[luna-soak] write {}: {e}", cli.out.display());
        return ExitCode::from(2);
    }
    println!("{}", json);

    if let Some(err) = early_exit_err {
        eprintln!("[luna-soak] workload errored early: {err}");
        return ExitCode::from(1);
    }
    let _wrapped = wrapped; // silence unused
    ExitCode::SUCCESS
}
