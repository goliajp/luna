//! `luna-repl-polish` — REPL polish (multi-line, completion,
//! `~/.luna_history` smarter handling).
//!
//! v2.0 Track TL Phase 2 stub. The impl pins `rustyline = "=14.x"`
//! per `.dev/rfcs/v2.0-audit-tl.md` § R3 (the 14→15 API break is a
//! mid-sprint hazard); the dep + impl land together in Phase 2.
//!
//! The current REPL surface lives in `luna` (the `luna-jit` bin
//! target). This polish binary is intentionally additive — once
//! shipped, embedders can pick the polish build via
//! `cargo install luna-tools --no-default-features --features
//! repl-polish` while the baseline `luna` binary stays
//! dep-light.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "luna-repl-polish",
    version,
    about = "Polished luna REPL (Phase 2 — stub)."
)]
struct Cli {
    /// Path to a history file (defaults to `~/.luna_history`).
    #[arg(long)]
    history: Option<std::path::PathBuf>,
}

fn main() {
    let _cli = Cli::parse();
    unimplemented!(
        "luna-repl-polish ships in v2.0 Track TL Phase 2 \
         (rustyline =14.x pin lands with impl) — \
         see .dev/rfcs/v2.0-plan-state.md § Track TL"
    );
}
