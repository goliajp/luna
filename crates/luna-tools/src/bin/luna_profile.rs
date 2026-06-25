//! `luna-profile` — flame-graph profiler over a luna `Vm` run.
//!
//! v2.0 Track TL Phase 2 stub. Ships in a follow-up commit; the
//! impl pulls `inferno` + `pprof` behind the `flame-graph` feature
//! per `.dev/rfcs/v2.0-audit-tl.md` § R2 (supply-chain isolation).
//!
//! When implemented the binary will:
//!   1. Spawn a `luna_jit::Vm` with the user-supplied script.
//!   2. Drive a sample-based profiler (pprof) that walks
//!      `luna_jit::inspect::frames_for_profile` on each tick.
//!   3. Fold sampled stacks and emit a flame-graph SVG via
//!      `inferno`'s flamegraph crate.
//!
//! The CLI surface is pinned today (`--script`, `--out`,
//! `--sample-hz`) so future fills don't break user muscle memory.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "luna-profile",
    version,
    about = "Flame-graph profiler for luna Vm runs (Phase 2 — stub)."
)]
struct Cli {
    /// Path to a `.lua` script to run under the profiler.
    #[arg(long)]
    script: std::path::PathBuf,
    /// Output SVG path for the flame-graph render.
    #[arg(long, default_value = "profile.svg")]
    out: std::path::PathBuf,
    /// Sampling frequency in Hz.
    #[arg(long, default_value_t = 99)]
    sample_hz: u32,
}

fn main() {
    let _cli = Cli::parse();
    unimplemented!(
        "luna-profile ships in v2.0 Track TL Phase 2 — \
         see .dev/rfcs/v2.0-plan-state.md § Track TL"
    );
}
