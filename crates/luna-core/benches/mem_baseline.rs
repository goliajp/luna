//! v2.0 Phase 5 Track MM — memory baseline bench.
//!
//! Measures `luna-core` heap usage across 5 representative workloads.
//! **Measurement-only** at this phase — no layout changes, no
//! `Vm::heap_stats()` API. Goal:
//!
//! 1. Establish reproducible peak / steady / alloc-count / alloc-byte
//!    numbers per workload so future Track MM layout attacks (Userdata
//!    io-field split, Node bitpack, StringTable load-factor,
//!    table_pool cap) can be A/B'd against ground truth.
//! 2. Surface surprising hot allocations the Phase 0 audit could only
//!    estimate.
//!
//! `dhat` (the heap profiler) is wired in as a `[dev-dependency]`, so
//! the prod `cargo tree -p luna-core --edges normal` still reports
//! exactly 1 crate. The CI `zero-dep` gate uses `--edges normal`
//! explicitly so dev-deps cannot regress the F1 0-third-party-dep
//! contract.
//!
//! ## Workloads (per audit Track MM §plan)
//!
//! - `cold_start`        — fresh `Vm::new` + 1 `eval("return 0")`
//! - `repl_idle`         — 100 simple eval statements, REPL-shape
//! - `host_roots_churn`  — 1000 `pin_host` / `unpin` cycles
//! - `alloc_collect`     — 1M `local x = {}` + 10 `collectgarbage()`
//! - `userdata_lifecycle`— 200 finalizable-table allocations + GC
//!
//! ## Run
//!
//! ```text
//! cargo bench --bench mem_baseline -p luna-core
//! ```
//!
//! The bench writes per-workload `.dhat` heap profiles + a one-line
//! summary per workload to stdout. Re-baseline:
//! `MM_DHAT_OUT=/path/to/dir cargo bench --bench mem_baseline -p luna-core`.
//! See `docs/contributing-mem.md` for full workflow.

use std::path::PathBuf;

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Output dir for `.dhat` profiles. Defaults to a sibling of the bench
/// binary; override with `MM_DHAT_OUT=...`.
fn dhat_out_dir() -> PathBuf {
    if let Ok(p) = std::env::var("MM_DHAT_OUT") {
        return PathBuf::from(p);
    }
    let mut p = std::env::temp_dir();
    p.push("luna-mem-baseline");
    p
}

/// Summary captured per workload. dhat's `HeapStats` lifetimes don't
/// outlive the profiler `Drop`, so we copy primitive fields out at
/// the end of the closure.
#[derive(Debug, Clone, Copy)]
struct Stats {
    /// Total bytes ever allocated during the workload window.
    total_bytes: u64,
    /// Total distinct allocation calls during the workload window.
    total_blocks: u64,
    /// Peak resident bytes (high-water mark) during the window.
    max_bytes: u64,
    /// Peak resident block count during the window.
    max_blocks: u64,
    /// Final resident bytes at end-of-window (steady).
    curr_bytes: u64,
    /// Final resident block count at end-of-window.
    curr_blocks: u64,
}

impl Stats {
    fn from_dhat(s: dhat::HeapStats) -> Self {
        Self {
            total_bytes: s.total_bytes,
            total_blocks: s.total_blocks,
            max_bytes: s.max_bytes as u64,
            max_blocks: s.max_blocks as u64,
            curr_bytes: s.curr_bytes as u64,
            curr_blocks: s.curr_blocks as u64,
        }
    }

    fn fmt_kb(&self) -> String {
        format!(
            "peak={:>9} B  steady={:>9} B  total={:>11} B  allocs={:>9}  peak_blocks={:>7}  steady_blocks={:>7}",
            self.max_bytes,
            self.curr_bytes,
            self.total_bytes,
            self.total_blocks,
            self.max_blocks,
            self.curr_blocks,
        )
    }
}

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Run `body` twice under fresh dhat profilers and produce two
/// outputs: (1) live `Stats` snapshot (requires dhat's `testing()`
/// mode — non-testing mode panics in `HeapStats::get()`), and (2) a
/// persistent `<out>/<name>.dhat.json` profile (requires non-testing
/// mode — testing mode never flushes to disk). dhat's single global
/// profiler can only be in one mode at a time, so we accept the cost
/// of running the workload twice in exchange for both artifacts.
///
/// `body` returns an owned value (typically the `Vm`) that the runner
/// holds until after the per-run snapshot, so cleanup allocations
/// don't pollute the profile and so `curr_bytes` ≠ 0.
fn run_workload<T, F: Fn() -> T>(name: &str, body: F) -> Stats {
    let out = dhat_out_dir();
    std::fs::create_dir_all(&out).expect("mkdir MM_DHAT_OUT");
    let path = out.join(format!("{name}.dhat.json"));

    // Pass 1 — testing mode, capture stats.
    let profiler = dhat::Profiler::builder().testing().build();
    let owned = body();
    let snap = Stats::from_dhat(dhat::HeapStats::get());
    drop(owned);
    drop(profiler);

    // Pass 2 — production mode, write .dhat.json for offline inspection.
    let profiler = dhat::Profiler::builder().file_name(&path).build();
    let owned = body();
    drop(owned);
    drop(profiler);

    println!("[mem_baseline] {name:>22}  {}", snap.fmt_kb());
    println!("[mem_baseline] {name:>22}  profile  {}", path.display());
    snap
}

// ──────────────────────────────────────────────────────────────────────
// Workload 1 — cold_start
// ──────────────────────────────────────────────────────────────────────
fn workload_cold_start() -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let _ = vm.eval("return 0").expect("cold eval");
    vm
}

// ──────────────────────────────────────────────────────────────────────
// Workload 2 — repl_idle (100 simple eval stmts, REPL pattern)
// ──────────────────────────────────────────────────────────────────────
fn workload_repl_idle() -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua55);
    for i in 0..100i64 {
        let src = format!("return {i}");
        let _ = vm.eval(&src).expect("repl eval");
    }
    vm
}

// ──────────────────────────────────────────────────────────────────────
// Workload 3 — host_roots_churn (1000 pin/unpin cycles)
// ──────────────────────────────────────────────────────────────────────
fn workload_host_roots_churn() -> Vm {
    use luna_core::runtime::value::Value;
    let mut vm = Vm::new(LuaVersion::Lua55);
    for i in 0..1000 {
        let t = vm.pin_host(Value::Int(i));
        vm.unpin(t).expect("host root unpin");
    }
    vm
}

// ──────────────────────────────────────────────────────────────────────
// Workload 4 — alloc_collect (1M `local x = {}` + 10 GCs)
// ──────────────────────────────────────────────────────────────────────
fn workload_alloc_collect() -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua55);
    // Tighter inner loop than 1M individual evals — eval overhead would
    // dwarf the table churn we are measuring. 10 iterations × 100
    // tables/iter for the GC-cadence portion, then one bulk eval for
    // the remaining ~999,000 tables to hit the audit's "1M tables"
    // headline.
    let src = r#"
        for i = 1, 100 do
            local x = {}
        end
        collectgarbage("collect")
    "#;
    for _ in 0..10 {
        let _ = vm.eval(src).expect("alloc_collect eval");
    }
    let bulk = r#"
        for i = 1, 999000 do
            local x = {}
        end
        collectgarbage("collect")
    "#;
    let _ = vm.eval(bulk).expect("alloc_collect bulk eval");
    vm
}

// ──────────────────────────────────────────────────────────────────────
// Workload 5 — userdata_lifecycle (200 finalizable tables + GC)
//
// luna's Userdata API is meant for embedder-side Rust types; for a
// pure-Lua heap-pressure proxy we use the `__gc` metamethod path,
// which is what the v1.2 LuaUserdata trait sugar lowers to under the
// hood. This exercises the same finalizer queue + extra GC pass that
// real Userdata allocations would hit.
// ──────────────────────────────────────────────────────────────────────
fn workload_userdata_lifecycle() -> Vm {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let src = r#"
        local mt = { __gc = function(_) end }
        for i = 1, 200 do
            local t = setmetatable({}, mt)
        end
        collectgarbage("collect")
        collectgarbage("collect")
    "#;
    let _ = vm.eval(src).expect("userdata_lifecycle eval");
    vm
}

fn main() {
    println!(
        "[mem_baseline] dhat profile dir: {}",
        dhat_out_dir().display()
    );
    println!("[mem_baseline] {:>22}  stats", "workload");

    let _w1 = run_workload("cold_start", workload_cold_start);
    let _w2 = run_workload("repl_idle", workload_repl_idle);
    let _w3 = run_workload("host_roots_churn", workload_host_roots_churn);
    let _w4 = run_workload("alloc_collect", workload_alloc_collect);
    let _w5 = run_workload("userdata_lifecycle", workload_userdata_lifecycle);

    println!("[mem_baseline] done");
}
