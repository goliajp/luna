//! `luna-aot` CLI surface — clap-derive. Sub-commands:
//!
//! - `luna-aot compile <input.lua> --out <output>` — the scaffold's
//!   only working command.
//!
//! Future v1.3-window commands (per `.dev/rfcs/v1.3-audit-luna-aot.md`):
//!
//! - `luna-aot run <input.lua>` — compile + execute in one shot
//!   (convenience; audit § Open question 7).
//! - `luna-aot dump <input.lua>` — print the dump bytes for
//!   inspection without touching the linker.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use luna_core::version::LuaVersion;

use crate::embed::{AotError, embed_bytecode};

/// `luna-aot` — Lua source → native binary compiler.
#[derive(Parser, Debug)]
#[command(name = "luna-aot", version, about, long_about = None)]
pub struct Cli {
    /// The sub-command to execute (currently only `compile`).
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level sub-commands. Only `compile` is wired in the scaffold;
/// the `Help` shape stays here so `luna-aot --help` lists what's
/// coming.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Compile a Lua source file into a native binary embedding the
    /// luna bytecode dump (scaffold: runs a tiny C entry that prints
    /// the embedded section size; follow-up wires the real Vm-driven
    /// runtime stub).
    Compile(CompileArgs),
}

/// Arguments for `luna-aot compile`.
#[derive(clap::Args, Debug)]
pub struct CompileArgs {
    /// Lua source file to compile.
    pub input: PathBuf,

    /// Output binary path. Defaults to the input's stem (e.g.
    /// `foo.lua` → `foo`).
    #[arg(short = 'o', long = "out")]
    pub out: Option<PathBuf>,

    /// Target triple. Scaffold only supports the host triple;
    /// anything else errors out at the `embed_bytecode` boundary.
    #[arg(long = "target")]
    pub target: Option<String>,

    /// Lua dialect. Defaults to 5.5 (matches the `luna` runner at
    /// `crates/luna-jit/src/bin/luna.rs`).
    #[arg(long = "dialect", default_value = "lua55")]
    pub dialect: DialectArg,
}

/// Dialect choice surface for the CLI. Mirrors
/// [`LuaVersion`] but is `clap::ValueEnum` so `--dialect 5.5` parses.
#[derive(ValueEnum, Copy, Clone, Debug)]
#[clap(rename_all = "lower")]
pub enum DialectArg {
    /// Lua 5.1
    #[clap(name = "5.1", alias = "lua51")]
    Lua51,
    /// Lua 5.2
    #[clap(name = "5.2", alias = "lua52")]
    Lua52,
    /// Lua 5.3
    #[clap(name = "5.3", alias = "lua53")]
    Lua53,
    /// Lua 5.4
    #[clap(name = "5.4", alias = "lua54")]
    Lua54,
    /// Lua 5.5 (default)
    #[clap(name = "5.5", alias = "lua55")]
    Lua55,
    /// MacroLua (5.4 base + compile-time macros, Phase ML).
    #[clap(name = "macrolua", alias = "macro")]
    MacroLua,
}

impl From<DialectArg> for LuaVersion {
    fn from(d: DialectArg) -> Self {
        match d {
            DialectArg::Lua51 => LuaVersion::Lua51,
            DialectArg::Lua52 => LuaVersion::Lua52,
            DialectArg::Lua53 => LuaVersion::Lua53,
            DialectArg::Lua54 => LuaVersion::Lua54,
            DialectArg::Lua55 => LuaVersion::Lua55,
            DialectArg::MacroLua => LuaVersion::MacroLua,
        }
    }
}

/// Entry point invoked from `main.rs`. Parses argv, dispatches to the
/// requested sub-command, prints errors to `stderr` and returns the
/// process exit code.
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Compile(args) => match do_compile(args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("luna-aot: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn do_compile(args: CompileArgs) -> Result<(), AotError> {
    let out = args.out.unwrap_or_else(|| default_out_for(&args.input));
    let version = args.dialect.into();
    embed_bytecode(&args.input, &out, args.target.as_deref(), version)
}

fn default_out_for(input: &std::path::Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("a")
        .to_string();
    let mut p = input
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    p.push(stem);
    p
}
