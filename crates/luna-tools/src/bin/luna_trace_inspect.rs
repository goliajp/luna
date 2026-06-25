//! `luna-trace-inspect` — live JIT trace introspection.
//!
//! v2.0 Track TL Phase 2 stub. The impl is gated on Track R's IR
//! overhaul stabilising per `.dev/rfcs/v2.0-audit-tl.md` § R1: any
//! `--show ir` output today would re-format after Track R lands.
//!
//! When implemented the binary will:
//!   1. Run a `.lua` script in a `luna_jit::Vm` with trace JIT on.
//!   2. After each closed trace, call
//!      `luna_jit::inspect::jit_state_snapshot` and dump:
//!        - `active_trace` head_pc + length
//!        - install / dispatch / abort counters
//!        - optionally (`--show ir`) the trace IR ops
//!        - optionally (`--show mcode`, requires
//!          `--features mcode-disasm`) capstone-disassembled
//!          Cranelift mcode for the trace body.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "luna-trace-inspect",
    version,
    about = "Live JIT trace state inspector (Phase 2 — stub)."
)]
struct Cli {
    /// Lua script to drive the trace recorder.
    #[arg(long)]
    script: std::path::PathBuf,
    /// What to show after each closed trace.
    #[arg(long, value_enum, default_value_t = ShowMode::Summary)]
    show: ShowMode,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ShowMode {
    /// Just counters + active-trace head_pc / length.
    Summary,
    /// Add the trace IR ops (gated on Track R IR shape).
    Ir,
    /// Add capstone-disassembled mcode (needs
    /// `--features mcode-disasm`).
    Mcode,
}

fn main() {
    let _cli = Cli::parse();
    unimplemented!(
        "luna-trace-inspect ships in v2.0 Track TL Phase 2 \
         (gated on Track R IR shape stabilising) — \
         see .dev/rfcs/v2.0-plan-state.md § Track TL"
    );
}
