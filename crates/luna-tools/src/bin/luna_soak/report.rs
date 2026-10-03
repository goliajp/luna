//! The JSON metrics report and the process RSS probe.

use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Sample {
    pub t_secs: u64,
    /// `memory_used()` of the Vm sampled; in `--vm-churn` mode, of the
    /// last Vm just before it was dropped
    pub vm_mem_used: usize,
    pub rss_kb: u64,
    /// Vms created so far (1 outside `--vm-churn`)
    pub vms: u64,
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
    /// from the middle sample to the last: the first half absorbs
    /// one-time growth (allocator arenas, lazily built tables)
    rss_mid_kb: u64,
    rss_second_half_drift_pct: f64,
}

#[derive(Debug, Serialize)]
struct Report<'a> {
    schema_version: u32,
    workload: String,
    mode: &'static str,
    duration_secs: u64,
    interval_secs: u64,
    samples: &'a [Sample],
    summary: Summary,
}

pub fn rss_kb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let s = fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
        let line = s
            .lines()
            .find_map(|l| l.strip_prefix("VmRSS:"))
            .expect("VmRSS line in /proc/self/status");
        line.split_whitespace()
            .next()
            .and_then(|w| w.parse().ok())
            .expect("VmRSS value")
    }
    // `ps -o rss=` is in KB; a ~10 ms fork+exec is noise at a >= 1 s
    // sampling interval, and it keeps libc out of luna-tools
    #[cfg(target_os = "macos")]
    {
        let pid = std::process::id();
        let out = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "rss="])
            .output()
            .expect("run ps");
        std::str::from_utf8(&out.stdout)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .expect("ps rss output")
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

fn pct<T: Ord + Copy>(mut xs: Vec<T>, p: usize) -> T {
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

pub struct Meta<'a> {
    pub workload: &'a Path,
    pub mode: &'static str,
    pub duration_secs: u64,
    pub interval_secs: u64,
}

/// Second-half RSS drift in percent of the samples so far.
pub fn second_half_rss_drift(samples: &[Sample]) -> f64 {
    let mid = samples[samples.len() / 2].rss_kb;
    drift_pct(mid as f64, samples[samples.len() - 1].rss_kb as f64)
}

/// Writes the full report for the samples collected so far; called after
/// every sample so a run killed from outside still leaves a complete
/// partial report for the artifact upload. Returns the JSON.
pub fn write_report(meta: &Meta, samples: &[Sample], out: &Path) -> std::io::Result<String> {
    let first = &samples[0];
    let last = &samples[samples.len() - 1];
    let summary = Summary {
        vm_mem_p50: pct(samples.iter().map(|s| s.vm_mem_used).collect(), 50),
        vm_mem_p99: pct(samples.iter().map(|s| s.vm_mem_used).collect(), 99),
        vm_mem_first: first.vm_mem_used,
        vm_mem_last: last.vm_mem_used,
        vm_mem_drift_pct: drift_pct(first.vm_mem_used as f64, last.vm_mem_used as f64),
        rss_p50_kb: pct(samples.iter().map(|s| s.rss_kb).collect(), 50),
        rss_p99_kb: pct(samples.iter().map(|s| s.rss_kb).collect(), 99),
        rss_first_kb: first.rss_kb,
        rss_last_kb: last.rss_kb,
        rss_drift_pct: drift_pct(first.rss_kb as f64, last.rss_kb as f64),
        rss_mid_kb: samples[samples.len() / 2].rss_kb,
        rss_second_half_drift_pct: second_half_rss_drift(samples),
    };
    let report = Report {
        schema_version: 2,
        workload: meta
            .workload
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string(),
        mode: meta.mode,
        duration_secs: meta.duration_secs,
        interval_secs: meta.interval_secs,
        samples,
        summary,
    };
    let json = serde_json::to_string_pretty(&report).expect("serializable");
    fs::write(out, &json)?;
    Ok(json)
}
