//! `luna-trace-inspect` — live JIT trace introspection.
//!
//! v2.0 Track TL Phase 2 ship. Runs a `.lua` script in a JIT-equipped
//! [`luna_jit::Vm`], then dumps the resulting [`luna_jit::inspect::
//! JitStateSnapshot`] so embedders can see how the trace JIT engaged.
//!
//! Output sections:
//! - **Counter summary** — closed / aborted / compiled / dispatched
//!   / deopt counts.
//! - **JIT enable bits** — master `enabled` + `trace_enabled`.
//! - **Active trace** — `head_pc` + recorded op-stream length, if a
//!   trace is still in flight at script-exit (rare; most workloads
//!   close their traces before the chunk returns).
//!
//! IR + mcode dumps are **intentionally deferred** per
//! `.dev/rfcs/v2.0-audit-tl.md` § R1: the IR shape will refactor in
//! Track R, so emitting it today would force a flag deprecation
//! within the same release line. The `--show` CLI surface still
//! pins `ir` / `mcode` as values so future fills don't break user
//! muscle memory — they currently exit non-zero with a clear pointer
//! to the tracking doc instead of pretending to render.
//!
//! Output formats: text (default, human-readable) or `--format json`
//! for downstream tooling. The JSON schema is intentionally narrow
//! today (just the [`luna_jit::inspect::JitStateSnapshot`] fields);
//! it can grow without breaking existing consumers thanks to
//! serde's additive-field default.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use luna_jit::inspect;
use luna_jit::version::LuaVersion;

#[derive(Debug, Parser)]
#[command(
    name = "luna-trace-inspect",
    version,
    about = "Live JIT trace state inspector — runs a script then dumps the JIT state."
)]
struct Cli {
    /// Lua script to drive the trace recorder.
    script: PathBuf,
    /// What to show after the script returns.
    #[arg(long, value_enum, default_value_t = ShowMode::Summary)]
    show: ShowMode,
    /// Output format. `text` is human-readable; `json` emits a
    /// schema-versioned dump of the [`luna_jit::inspect::
    /// JitStateSnapshot`] for downstream tooling.
    #[arg(long, value_enum, default_value_t = OutMode::Text)]
    format: OutMode,
    /// Lua dialect; defaults to 5.5 (luna's primary dialect).
    #[arg(long, value_enum, default_value_t = Dialect::Lua55)]
    dialect: Dialect,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ShowMode {
    /// Just counters + active-trace head_pc / length + enable bits.
    Summary,
    /// Add the trace IR ops — **currently deferred** to Track R IR
    /// overhaul stabilising; using this flag exits non-zero with a
    /// pointer to the tracking doc.
    Ir,
    /// Add capstone-disassembled mcode — **currently deferred** to
    /// the `--features mcode-disasm` capstone wrapper; using this
    /// flag exits non-zero with a pointer to the tracking doc.
    Mcode,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutMode {
    Text,
    Json,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Dialect {
    Lua51,
    Lua52,
    Lua53,
    Lua54,
    Lua55,
}

impl Dialect {
    fn to_version(self) -> LuaVersion {
        match self {
            Dialect::Lua51 => LuaVersion::Lua51,
            Dialect::Lua52 => LuaVersion::Lua52,
            Dialect::Lua53 => LuaVersion::Lua53,
            Dialect::Lua54 => LuaVersion::Lua54,
            Dialect::Lua55 => LuaVersion::Lua55,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("luna-trace-inspect: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    // R1 gate: IR / mcode shapes will refactor in Track R; refuse
    // them up-front rather than print misleading output.
    match cli.show {
        ShowMode::Summary => {}
        ShowMode::Ir => {
            return Err("--show ir is reserved for Track R IR shape stabilising; \
                 see .dev/rfcs/v2.0-plan-state.md § Track TL audit R1"
                .into());
        }
        ShowMode::Mcode => {
            return Err("--show mcode needs `--features mcode-disasm` (capstone); \
                 see .dev/rfcs/v2.0-plan-state.md § Track TL audit R1"
                .into());
        }
    }

    let src = std::fs::read_to_string(&cli.script)
        .map_err(|e| format!("reading {}: {e}", cli.script.display()))?;
    let mut vm = luna_jit::new_with_jit(cli.dialect.to_version());
    vm.eval_chunk(&src, &cli.script.display().to_string())
        .map_err(|e| format!("running script: {e:?}"))?;

    let snap = inspect::jit_state_snapshot(&vm);
    match cli.format {
        OutMode::Text => print_text(&snap),
        OutMode::Json => {
            let payload = serde_json::json!({
                "schema": "luna-trace-inspect.v1",
                "luna_version": env!("CARGO_PKG_VERSION"),
                "enabled": snap.enabled,
                "trace_enabled": snap.trace_enabled,
                "active_trace_head_pc": snap.active_trace_head_pc,
                "active_trace_len": snap.active_trace_len,
                "counters": {
                    "trace_compiled": snap.trace_compiled_count,
                    "trace_closed": snap.trace_closed_count,
                    "trace_aborted": snap.trace_aborted_count,
                    "trace_dispatched": snap.trace_dispatched_count,
                    "trace_deopt": snap.trace_deopt_count,
                },
            });
            let s = serde_json::to_string_pretty(&payload)
                .map_err(|e| format!("serializing JSON: {e}"))?;
            println!("{s}");
        }
    }
    Ok(())
}

fn print_text(snap: &inspect::JitStateSnapshot) {
    println!("luna-trace-inspect (luna {})", env!("CARGO_PKG_VERSION"));
    println!("  jit enabled       : {}", snap.enabled);
    println!("  trace enabled     : {}", snap.trace_enabled);
    println!("  counters:");
    println!("    compiled    : {}", snap.trace_compiled_count);
    println!("    closed      : {}", snap.trace_closed_count);
    println!("    aborted     : {}", snap.trace_aborted_count);
    println!("    dispatched  : {}", snap.trace_dispatched_count);
    println!("    deopt       : {}", snap.trace_deopt_count);
    match (snap.active_trace_head_pc, snap.active_trace_len) {
        (Some(pc), Some(len)) => {
            println!("  active trace      : head_pc={pc} ops_len={len}");
        }
        _ => {
            println!("  active trace      : <none>");
        }
    }
}
