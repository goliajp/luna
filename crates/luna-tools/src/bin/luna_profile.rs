//! `luna-profile` — sampling profiler for luna `Vm` runs.
//!
//! v2.0 Track TL Phase 2 ship. Drives the script under a Count debug
//! hook that fires every N instructions; each fire walks the current
//! Lua call stack via [`luna_jit::inspect::frames_for_profile`] and
//! folds the resulting stack trace into an in-memory histogram. At
//! script-exit the histogram dumps as a flat profile (default) or as
//! folded-stack lines for `inferno-flamegraph` consumers.
//!
//! Output formats:
//! - `text` (default) — top-N hottest stacks + sample count + %.
//! - `folded` — one folded-stack line per histogram entry, in the
//!   `frame_a;frame_b;frame_c N` shape that `inferno-flamegraph`
//!   reads on stdin. Pipe through `inferno-flamegraph` to render
//!   the SVG; we don't bundle that step because `inferno`'s
//!   ~30-crate transitive closure is a `--features flame-graph`
//!   opt-in per audit R2.
//! - `pprof` — reserved; gated on `--features flame-graph` per audit
//!   R2 (`pprof` crate has a `prost`-shaped closure). Currently
//!   exits non-zero with a tracking pointer.
//!
//! No `unsafe` introduced at the embedder surface — the sampler is
//! a fn pointer through luna_core's existing B11 Rust-side debug
//! hook (`set_rust_debug_hook`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Mutex;

use clap::Parser;
use luna_jit::inspect::{self, FrameInfo};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::exec::{HOOK_MASK_COUNT, RustHookEvent};

#[derive(Debug, Parser)]
#[command(
    name = "luna-profile",
    version,
    about = "Sampling profiler for luna Vm runs (text + folded-stack output)."
)]
struct Cli {
    /// Lua script to run under the profiler.
    script: PathBuf,
    /// Sample every N instructions. Smaller N = denser sampling at
    /// higher hook overhead. Default 1024 ≈ ~1KHz on a 1MHz dispatch
    /// loop, which is plenty for hot-path identification without
    /// drowning the script.
    #[arg(long, default_value_t = 1024, value_name = "N")]
    every: i64,
    /// Output format.
    #[arg(long, value_enum, default_value_t = OutMode::Text)]
    format: OutMode,
    /// For text output, show the top-N hottest stacks. Ignored for
    /// `folded` and `pprof`.
    #[arg(long, default_value_t = 20)]
    top: usize,
    /// Lua dialect; defaults to 5.5 (luna's primary dialect).
    #[arg(long, value_enum, default_value_t = Dialect::Lua55)]
    dialect: Dialect,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum OutMode {
    /// Human-readable top-N hot frames + counts + percentage.
    Text,
    /// `frame_a;frame_b;frame_c N` lines for `inferno-flamegraph`.
    Folded,
    /// pprof protobuf — gated on `--features flame-graph`. Currently
    /// exits non-zero with a tracking pointer per audit R2.
    Pprof,
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

/// One stack-trace key in the sample histogram. Owned so the
/// HashMap can outlive any single hook tick's borrow on the Vm.
type StackKey = Vec<String>;

/// Histogram of (stack-trace, sample-count) accumulated by the
/// Count hook. `Mutex` for sync; the hook runs single-threaded
/// under the Vm dispatcher but we route through a static
/// container so the fn-pointer hook can reach it.
static HISTOGRAM: Mutex<Option<HashMap<StackKey, u64>>> = Mutex::new(None);

fn sampler_hook(vm: &mut Vm, event: RustHookEvent) {
    // Subscribed only to Count; defensive match so a future mask
    // change can't sneak in.
    if !matches!(event, RustHookEvent::Count) {
        return;
    }
    let frames = inspect::frames_for_profile(vm);
    if frames.is_empty() {
        return;
    }
    let key: StackKey = frames.iter().map(format_frame).collect();
    let mut guard = HISTOGRAM.lock().unwrap();
    let Some(hist) = guard.as_mut() else { return };
    *hist.entry(key).or_insert(0) += 1;
}

fn format_frame(f: &FrameInfo) -> String {
    // `source:line` is the standard PUC short-traceback frame
    // shape; folded-stack consumers (flamegraph.pl, inferno) split
    // on `;`, so we don't introduce any other punctuation that
    // could collide.
    if f.line == 0 {
        format!("{}:?", f.source)
    } else {
        format!("{}:{}", f.source, f.line)
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("luna-profile: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    if matches!(cli.format, OutMode::Pprof) {
        return Err(
            "--format pprof needs `--features flame-graph` (pprof + prost); \
             see .dev/rfcs/v2.0-plan-state.md § Track TL audit R2"
                .into(),
        );
    }
    if cli.every <= 0 {
        return Err(format!("--every must be > 0, got {}", cli.every));
    }

    let src = std::fs::read_to_string(&cli.script)
        .map_err(|e| format!("reading {}: {e}", cli.script.display()))?;

    // Reset histogram (in case a prior process reused the slot).
    *HISTOGRAM.lock().unwrap() = Some(HashMap::new());

    let mut vm = luna_jit::new_with_jit(cli.dialect.to_version());
    vm.set_rust_debug_hook(Some(sampler_hook), HOOK_MASK_COUNT, cli.every);

    vm.eval_chunk(&src, &cli.script.display().to_string())
        .map_err(|e| format!("running script: {e:?}"))?;

    // Detach the hook before draining so a later GC / drop on Vm
    // doesn't fire the histogram path again.
    vm.clear_rust_debug_hook();

    let hist = HISTOGRAM
        .lock()
        .unwrap()
        .take()
        .unwrap_or_default();
    emit(cli, hist);
    Ok(())
}

fn emit(cli: &Cli, hist: HashMap<StackKey, u64>) {
    let total: u64 = hist.values().sum();
    let mut rows: Vec<(StackKey, u64)> = hist.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    match cli.format {
        OutMode::Folded => {
            for (stack, count) in &rows {
                // Reverse the stack so the root frame appears first
                // (inferno-flamegraph convention: leftmost = root).
                let folded = stack
                    .iter()
                    .rev()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(";");
                println!("{folded} {count}");
            }
        }
        OutMode::Text => {
            println!("luna-profile (luna {})", env!("CARGO_PKG_VERSION"));
            println!(
                "  samples: {} (every={} insts), distinct stacks: {}",
                total,
                cli.every,
                rows.len()
            );
            if total == 0 {
                println!(
                    "  <no samples — script may have been too short for one \
                     hook tick; try --every 1 or a longer workload>"
                );
                return;
            }
            println!("  top {}:", cli.top.min(rows.len()));
            println!("    {:>8} {:>6}  STACK (leaf → root)", "COUNT", "PCT");
            for (stack, count) in rows.iter().take(cli.top) {
                let pct = (*count as f64) * 100.0 / (total as f64);
                let render = stack.join(" / ");
                println!("    {count:>8} {pct:>5.1}%  {render}");
            }
        }
        OutMode::Pprof => unreachable!("pprof guarded above"),
    }
}
