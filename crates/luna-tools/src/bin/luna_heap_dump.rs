//! `luna-heap-dump` — run a Lua script and emit a snapshot of the
//! resulting heap state.
//!
//! v2.0 Track TL Phase 1 ship. Uses the pure-read inspection
//! accessors in [`luna_jit::inspect`] — no private fields touched,
//! no allocations on the hot path of the running script.
//!
//! Unified-with-MM-track note: per `.dev/rfcs/v2.0-audit-tl.md`,
//! heap-dump shares its snapshot schema ([`luna_tools::schema::
//! HeapSnapshot`]) with the future `luna-heap-diff` tool that Track
//! MM will land — both sides parse the same JSON.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use luna_jit::inspect;
use luna_jit::version::LuaVersion;
use luna_tools::schema::{HeapSnapshot, HeapTypeBucket, LUNA_TOOLS_SCHEMA_VERSION};

#[derive(Debug, Parser)]
#[command(
    name = "luna-heap-dump",
    version,
    about = "Run a Lua script and dump a per-type heap snapshot."
)]
struct Cli {
    /// Lua script to run.
    script: PathBuf,
    /// Output format. `text` is a human-readable table; `json`
    /// emits the [`luna_tools::schema::HeapSnapshot`] schema for
    /// downstream tools (e.g. the future `luna-heap-diff`).
    #[arg(long, value_enum, default_value_t = OutMode::Text)]
    out: OutMode,
    /// Lua dialect; defaults to 5.5 (luna's primary dialect).
    #[arg(long, value_enum, default_value_t = Dialect::Lua55)]
    dialect: Dialect,
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
            eprintln!("luna-heap-dump: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let src = std::fs::read_to_string(&cli.script)
        .map_err(|e| format!("reading {}: {e}", cli.script.display()))?;
    let mut vm = luna_jit::new_with_jit(cli.dialect.to_version());
    vm.eval_chunk(&src, &cli.script.display().to_string())
        .map_err(|e| format!("running script: {e:?}"))?;

    let snap = inspect::heap_walk(&vm);
    let report = HeapSnapshot {
        schema_version: LUNA_TOOLS_SCHEMA_VERSION,
        luna_version: env!("CARGO_PKG_VERSION").to_string(),
        total_objects: snap.total_objects as u64,
        total_bytes: snap.total_bytes as u64,
        buckets: snap
            .buckets
            .into_iter()
            .map(|b| HeapTypeBucket {
                type_name: b.type_name.to_string(),
                count: b.count as u64,
                bytes_approx: b.bytes_approx as u64,
            })
            .collect(),
    };

    match cli.out {
        OutMode::Json => {
            let s = serde_json::to_string_pretty(&report)
                .map_err(|e| format!("serializing JSON: {e}"))?;
            println!("{s}");
        }
        OutMode::Text => print_text(&report),
    }
    Ok(())
}

fn print_text(r: &HeapSnapshot) {
    println!("luna-heap-dump (luna {})", r.luna_version);
    println!(
        "  total: {} objects, {} bytes (approx, shells only)",
        r.total_objects, r.total_bytes
    );
    println!("  buckets:");
    println!("    {:<16} {:>10} {:>14}", "TYPE", "COUNT", "BYTES_APPROX");
    if r.buckets.is_empty() {
        println!("    <empty>");
        return;
    }
    for b in &r.buckets {
        println!(
            "    {:<16} {:>10} {:>14}",
            b.type_name, b.count, b.bytes_approx
        );
    }
}
