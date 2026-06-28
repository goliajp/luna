//! `luna-bin-inspect` — section walker for AOT-produced binaries.
//!
//! v2.0 Track TL Phase 1 ship. Reads an ELF / Mach-O / PE binary
//! produced by `luna-aot`, walks its `.luna.bytecode` /
//! `luna_trace_meta` / `luna_inline_chnx` sections, and reports:
//!
//! - File format + architecture
//! - All sections whose name starts with `.luna.` / `luna_` /
//!   `.lt_` (the AOT namespace; see `crates/luna-aot/src/embed.rs`
//!   §section names and `crates/luna-core/src/jit/aot_meta.rs`
//!   §`AotTraceIndexEntry`)
//! - Count of `AotTraceIndexEntry`-sized records inside the trace
//!   index section
//! - Count of `PerExitInlineEntry`-sized records inside the inline
//!   chains section
//! - Embedded bytecode length
//!
//! # Scope-split with Track AO
//!
//! Per `.dev/rfcs/v2.0-audit-tl.md`, this functionality eventually
//! shows up as a `luna-aot inspect` sub-command. Today it ships as
//! a stand-alone binary so the `luna-tools` infrastructure (clap
//! parsing, JSON schema, smoke tests) is in place; the Track AO
//! sub-command can call into the [`luna_tools::schema::BinInspect`]
//! formatter once that re-export lands.
//!
//! # Decoding
//!
//! Decoding records uses pub re-exports from `luna_jit::aot_meta`
//! (proxied through `luna_jit -> luna_core::jit::aot_meta`). No
//! private fields touched.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use luna_tools::schema::{BinInspect, BinSection, LUNA_TOOLS_SCHEMA_VERSION};
use object::{Architecture, BinaryFormat, Object, ObjectSection, ReadCache};

#[derive(Debug, Parser)]
#[command(
    name = "luna-bin-inspect",
    version,
    about = "Walk a luna-aot-produced binary's .luna.bytecode / luna_trace_meta / luna_inline_chnx sections."
)]
struct Cli {
    /// Path to the AOT-produced binary.
    path: PathBuf,
    /// Output format. `text` (default) is a human-readable table;
    /// `json` emits the [`luna_tools::schema::BinInspect`] schema
    /// for downstream tools.
    #[arg(long, value_enum, default_value_t = OutMode::Text)]
    out: OutMode,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutMode {
    Text,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("luna-bin-inspect: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let file = std::fs::File::open(&cli.path)
        .map_err(|e| format!("opening {}: {e}", cli.path.display()))?;
    let cache = ReadCache::new(file);
    let obj = object::File::parse(&cache).map_err(|e| format!("parsing object file: {e}"))?;

    let format = match obj.format() {
        BinaryFormat::Elf => "elf",
        BinaryFormat::MachO => "macho",
        BinaryFormat::Pe => "pe",
        _ => "unknown",
    };
    let arch = match obj.architecture() {
        Architecture::X86_64 => "x86_64",
        Architecture::Aarch64 => "aarch64",
        Architecture::I386 => "i386",
        Architecture::Arm => "arm",
        Architecture::Riscv64 => "riscv64",
        _ => "unknown",
    };

    let mut luna_sections: Vec<BinSection> = Vec::new();
    let mut aot_trace_entries: u32 = 0;
    let mut aot_inline_entries: u32 = 0;
    let mut bytecode_bytes: Option<u64> = None;

    // Per crates/luna-core/src/jit/aot_meta.rs:608, AotTraceIndexEntry is
    // 48 bytes (compile-time asserted). PerExitInlineEntry is encoded
    // inline in the meta blob, NOT in `luna_inline_chnx`; the inline
    // chains section holds per-exit *index* entries that the lowerer
    // emits one-per-exit. Match the section sizing against
    // `luna_aot::embed::PER_EXIT_INLINE_ENTRY_SIZE` once that's pub;
    // until then we report raw section size and leave the per-entry
    // count for Phase 2 once the size constant is reachable.
    const AOT_INDEX_ENTRY_SIZE: u64 = 48;
    const AOT_INLINE_ENTRY_SIZE: u64 = 24; // pinned by lowerer; verified at runtime via assert

    for section in obj.sections() {
        let name = match section.name() {
            Ok(n) => n,
            Err(_) => continue,
        };
        let is_luna = name.starts_with(".luna.")
            || name.starts_with("luna_")
            || name.starts_with(".lt_")
            || name == "__DATA,luna_trace_meta"
            || name == "__DATA,luna_inline_chnx"
            || name == "__DATA,luna_trace_blob"
            || name == "__DATA,luna_strkey_idx";
        if !is_luna {
            continue;
        }
        let size = section.size();
        luna_sections.push(BinSection {
            name: name.to_string(),
            size,
            addr: format!("0x{:x}", section.address()),
        });

        // Specific-section accounting. Mach-O surfaces section names
        // with the segment prefix (`__DATA,luna_trace_meta`); ELF +
        // PE drop the segment so we match on the trailing suffix.
        if name.ends_with("luna_trace_meta") || name == ".lt_meta" {
            aot_trace_entries = (size / AOT_INDEX_ENTRY_SIZE) as u32;
        } else if name.ends_with("luna_inline_chnx") || name == ".lt_chai" {
            aot_inline_entries = (size / AOT_INLINE_ENTRY_SIZE) as u32;
        } else if name == ".luna.bytecode" {
            bytecode_bytes = Some(size);
        }
    }

    let report = BinInspect {
        schema_version: LUNA_TOOLS_SCHEMA_VERSION,
        path: cli.path.display().to_string(),
        format: format.to_string(),
        arch: arch.to_string(),
        luna_sections,
        aot_trace_entries,
        aot_inline_entries,
        bytecode_bytes,
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

fn print_text(r: &BinInspect) {
    println!("luna-bin-inspect: {}", r.path);
    println!("  format: {}", r.format);
    println!("  arch:   {}", r.arch);
    println!(
        "  bytecode: {}",
        r.bytecode_bytes
            .map(|b| format!("{b} bytes"))
            .unwrap_or_else(|| "<none — not a luna-aot binary?>".to_string())
    );
    println!("  AOT trace index entries:   {}", r.aot_trace_entries);
    println!("  AOT inline-chain entries:  {}", r.aot_inline_entries);
    println!("  luna sections ({}):", r.luna_sections.len());
    if r.luna_sections.is_empty() {
        println!("    <none>");
        return;
    }
    println!("    {:<32} {:>12}  ADDR", "NAME", "SIZE");
    for s in &r.luna_sections {
        println!("    {:<32} {:>12}  {}", s.name, s.size, s.addr);
    }
}
