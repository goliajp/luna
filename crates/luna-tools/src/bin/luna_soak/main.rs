//! `luna-soak` — long-running workload runner with RSS + Vm memory
//! sampling.
//!
//! Reads a Lua workload file + duration, evaluates it, and samples runtime
//! metrics at a configurable interval. The output JSON is consumed by the
//! `.github/workflows/soak-nightly.yml` (1h smoke) and
//! `.github/workflows/soak-weekly.yml` (5h30m sample) workflows.
//!
//! Two modes:
//!
//! - default: one Vm runs the workload in a loop for the whole duration
//! - `--vm-churn`: every iteration creates a JIT Vm, runs the workload
//!   until the method JIT and the trace JIT have both compiled code, and
//!   drops the Vm, so what a dropped Vm leaks accumulates in RSS
//!
//! `--max-second-half-rss-drift-pct` turns the report into a check: the
//! run exits 1 when RSS grew more than that from the middle sample to the
//! last.
//!
//! # Usage
//!
//! ```sh
//! luna-soak --workload crates/luna-tools/workloads/token_bucket_1k.lua \
//!           --duration 60 --interval 1 --out /tmp/soak.json
//! luna-soak --vm-churn --workload crates/luna-tools/workloads/vm_churn.lua \
//!           --duration 600 --interval 10 --max-second-half-rss-drift-pct 1
//! ```
//!
//! # Output schema (version 2)
//!
//! `{"schema_version", "workload", "mode", "duration_secs",
//! "interval_secs", "samples": [{"t_secs", "vm_mem_used", "rss_kb",
//! "vms"}], "summary": {p50/p99/first/last/drift of vm_mem and rss,
//! "rss_mid_kb", "rss_second_half_drift_pct"}}`
//!
//! RSS comes from `/proc/self/status` (`VmRSS:`) on Linux and
//! `ps -o rss=` on macOS; other platforms report 0.

mod churn;
mod report;

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use clap::Parser;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

use report::{Meta, Sample, rss_kb, second_half_rss_drift, write_report};

#[derive(Debug, Parser)]
#[command(
    name = "luna-soak",
    version,
    about = "Run a Lua workload long-running with RSS + Vm memory sampling."
)]
struct Cli {
    /// Lua source file containing the workload. Each pass runs the chunk
    /// once; the runner repeats it for --duration.
    #[arg(long)]
    workload: PathBuf,

    /// How long to run the workload, in seconds.
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Sampling interval in seconds.
    #[arg(long, default_value_t = 1)]
    interval: u64,

    /// Soft cap on each Vm's memory usage in MiB. A workload that exceeds
    /// it raises a catchable "memory cap exceeded" Lua error and the soak
    /// exits early.
    #[arg(long, default_value_t = 128)]
    mem_cap_mib: usize,

    /// Create, JIT-compile and drop a fresh Vm per iteration instead of
    /// running one Vm for the whole duration.
    #[arg(long)]
    vm_churn: bool,

    /// Exit 1 when RSS grows more than this many percent from the middle
    /// sample to the last.
    #[arg(long)]
    max_second_half_rss_drift_pct: Option<f64>,

    /// Output path for the JSON metrics report.
    #[arg(long, default_value = "soak.json")]
    out: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let src = match fs::read_to_string(&cli.workload) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[luna-soak] read {}: {e}", cli.workload.display());
            return ExitCode::from(2);
        }
    };
    let meta = Meta {
        workload: &cli.workload,
        mode: if cli.vm_churn {
            "vm-churn"
        } else {
            "single-vm"
        },
        duration_secs: cli.duration,
        interval_secs: cli.interval,
    };
    let mut samples = Vec::new();
    let mut record = |sample: Sample| -> Result<String, String> {
        samples.push(sample);
        write_report(&meta, &samples, &cli.out)
            .map_err(|e| format!("write {}: {e}", cli.out.display()))
    };
    let run = if cli.vm_churn {
        run_churn(&cli, &src, &mut record)
    } else {
        run_single(&cli, &src, &mut record)
    };
    let (json, workload_err) = match run {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[luna-soak] {e}");
            return ExitCode::from(2);
        }
    };
    if let Some(json) = json {
        println!("{json}");
    }
    if let Some(err) = workload_err {
        eprintln!("[luna-soak] workload errored early: {err}");
        return ExitCode::from(1);
    }
    if let Some(max) = cli.max_second_half_rss_drift_pct {
        let drift = second_half_rss_drift(&samples);
        if drift >= max {
            eprintln!("[luna-soak] second-half RSS drift {drift:.3}% >= {max}%");
            return ExitCode::from(1);
        }
        eprintln!("[luna-soak] second-half RSS drift {drift:.3}% < {max}%");
    }
    ExitCode::SUCCESS
}

/// The last report written (none if the workload failed before the first
/// sample), and the workload's error if it stopped early.
type RunResult = Result<(Option<String>, Option<String>), String>;

/// One Vm runs `src` in timed slices; a sample is taken after each slice.
fn run_single(
    cli: &Cli,
    src: &str,
    record: &mut impl FnMut(Sample) -> Result<String, String>,
) -> RunResult {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(cli.mem_cap_mib * 1024 * 1024));
    let start = Instant::now();
    let mut json = Some(record(Sample {
        t_secs: 0,
        vm_mem_used: vm.memory_used(),
        rss_kb: rss_kb(),
        vms: 1,
    })?);
    let burst = cli.interval.max(1);
    let slice = format!(
        "local _slice_deadline = os.clock() + {burst}\n\
         while os.clock() < _slice_deadline do\n\
         {src}\n\
         end\n"
    );
    for _ in 0..(cli.duration / burst).max(1) {
        if let Err(e) = vm.eval(&slice) {
            return Ok((json, Some(format!("{e}"))));
        }
        let elapsed = start.elapsed().as_secs();
        json = Some(record(Sample {
            t_secs: elapsed,
            vm_mem_used: vm.memory_used(),
            rss_kb: rss_kb(),
            vms: 1,
        })?);
        if elapsed >= cli.duration {
            break;
        }
        // a slice that returns at once must not spin the sampler
        thread::sleep(Duration::from_millis(50));
    }
    Ok((json, None))
}

/// Fresh JIT Vms run `src` back to back; a sample is taken between two
/// Vms whenever an interval has passed.
fn run_churn(
    cli: &Cli,
    src: &str,
    record: &mut impl FnMut(Sample) -> Result<String, String>,
) -> RunResult {
    let interval = Duration::from_secs(cli.interval.max(1));
    let duration = Duration::from_secs(cli.duration);
    let mem_cap = cli.mem_cap_mib * 1024 * 1024;
    let start = Instant::now();
    // the first sample follows the first Vm: before it there is no Vm
    // whose memory could be reported
    let mut json = None;
    let mut vms = 0u64;
    let mut next_sample = Duration::ZERO;
    loop {
        let vm_mem_used = match churn::one_vm(src, mem_cap) {
            Ok(m) => m,
            Err(e) => return Ok((json, Some(format!("Vm #{}: {e}", vms + 1)))),
        };
        vms += 1;
        let elapsed = start.elapsed();
        if elapsed >= next_sample || elapsed >= duration {
            json = Some(record(Sample {
                t_secs: elapsed.as_secs(),
                vm_mem_used,
                rss_kb: rss_kb(),
                vms,
            })?);
            next_sample += interval;
        }
        if elapsed >= duration {
            return Ok((json, None));
        }
    }
}
