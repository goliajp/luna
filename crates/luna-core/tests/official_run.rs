// own binary: it changes the process cwd, and ci reads its stderr trace under --nocapture

//! Official PUC Lua test-suite gates for every supported dialect.
//!
//! For each Lua version luna implements we vendor PUC's released test tarball
//! and run a curated set of files end-to-end under `_U` (user) mode. Each
//! version's `expected_pass` list is the inventory of files that must pass
//! that dialect — promote a file into it once it runs all-green. `excluded`
//! documents the still-failing ones so the gate's scope is explicit.
//!
//! All suites share the process-global cwd (require's `./?.lua` searcher
//! resolves siblings relative to it), so the suites run sequentially inside a
//! single `#[test]` rather than as separate test functions racing for the
//! current directory.
//!
//! # Assert-coverage instrumentation
//!
//! Every PUC chunk is prepended with a single-line Lua snippet that wraps
//! `_G.assert` to bump two integer counters (`__luna_assert_total`,
//! `__luna_assert_hit`). After the chunk completes (or errors) the
//! counters are read back from the Vm globals and accumulated into a
//! per-file report written to the workspace's `target/` directory.
//! The report exposes which `_port` / `_soft` / `_noposix` gates are
//! silently skipping large blocks of `assert(...)` calls so future scope
//! decisions are evidence-based. The wrapper sits at file scope and
//! forwards every argument unchanged, so underlying assert semantics are
//! preserved. The wrapper itself is invisible to the counters (it does
//! not call `_a` recursively, only forwards).

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// Per-file assert-counter result captured by `run_file`.
#[derive(Debug, Clone)]
struct FileCoverage {
    version: LuaVersion,
    file: String,
    total: i64,
    hit: i64,
    /// `Some(err)` if the chunk ended with an error; coverage rows for
    /// failing files are still emitted to make partial-execution visible.
    error: Option<String>,
    /// `true` when the wrapper was intentionally not injected because the
    /// file introspects `assert` / `debug` in ways the wrapper can't
    /// faithfully replicate (`errors.lua`, `db.lua`).
    wrapper_skipped: bool,
}

/// One version's test gate: the suite directory (relative to the workspace
/// root) and the files that must run clean under that dialect.
struct Suite {
    version: LuaVersion,
    dir: &'static str,
    expected_pass: &'static [&'static str],
}

const SUITES: &[Suite] = &[
    Suite {
        version: LuaVersion::Lua55,
        dir: "tests/official/lua-5.5.1-tests",
        expected_pass: &[
            "main.lua",
            "api.lua",
            "attrib.lua",
            "big.lua",
            "bitwise.lua",
            "bwcoercion.lua",
            "calls.lua",
            "closure.lua",
            "code.lua",
            "constructs.lua",
            "coroutine.lua",
            "cstack.lua",
            "db.lua",
            "errors.lua",
            "events.lua",
            // 5.5 files.lua :474 needs a real `/dev/full` (Linux-only) to
            // probe the write-failure path; macOS has no such device.
            #[cfg(target_os = "linux")]
            "files.lua",
            "gc.lua",
            "gengc.lua",
            "goto.lua",
            "heavy.lua",
            "literals.lua",
            "locals.lua",
            "math.lua",
            "memerr.lua",
            "nextvar.lua",
            "pm.lua",
            "sort.lua",
            "strings.lua",
            "tpack.lua",
            "tracegc.lua",
            "utf8.lua",
            "vararg.lua",
            "verybig.lua",
        ],
    },
    Suite {
        version: LuaVersion::Lua54,
        dir: "tests/official/lua-5.4.9-tests",
        expected_pass: &[
            "verybig.lua",
            "main.lua",
            "api.lua",
            "attrib.lua",
            "big.lua",
            "bitwise.lua",
            "bwcoercion.lua",
            "calls.lua",
            "closure.lua",
            "code.lua",
            "constructs.lua",
            "coroutine.lua",
            "cstack.lua",
            "db.lua",
            "errors.lua",
            "events.lua",
            "files.lua",
            "gc.lua",
            "gengc.lua",
            "goto.lua",
            "heavy.lua",
            "literals.lua",
            "locals.lua",
            "math.lua",
            "nextvar.lua",
            "pm.lua",
            "sort.lua",
            "strings.lua",
            "tpack.lua",
            "tracegc.lua",
            "utf8.lua",
            "vararg.lua",
        ],
    },
    Suite {
        version: LuaVersion::Lua53,
        dir: "tests/official/lua-5.3.4-tests",
        expected_pass: &[
            "verybig.lua",
            "main.lua",
            "api.lua",
            "attrib.lua",
            "big.lua",
            "bitwise.lua",
            "calls.lua",
            "closure.lua",
            "code.lua",
            "constructs.lua",
            "coroutine.lua",
            "db.lua",
            "errors.lua",
            "events.lua",
            "files.lua",
            "gc.lua",
            "goto.lua",
            "literals.lua",
            "locals.lua",
            "math.lua",
            "nextvar.lua",
            "pm.lua",
            "sort.lua",
            "strings.lua",
            "tpack.lua",
            "utf8.lua",
            "vararg.lua",
        ],
    },
    Suite {
        version: LuaVersion::Lua52,
        dir: "tests/official/lua-5.2.2-tests",
        expected_pass: &[
            "verybig.lua",
            "main.lua",
            "api.lua",
            "attrib.lua",
            "big.lua",
            "bitwise.lua",
            "calls.lua",
            "checktable.lua",
            "closure.lua",
            "code.lua",
            "constructs.lua",
            "coroutine.lua",
            "db.lua",
            "errors.lua",
            "events.lua",
            "files.lua",
            "gc.lua",
            "goto.lua",
            "literals.lua",
            "locals.lua",
            "math.lua",
            "nextvar.lua",
            "pm.lua",
            "sort.lua",
            "strings.lua",
            "vararg.lua",
        ],
    },
    Suite {
        version: LuaVersion::Lua51,
        dir: "tests/official/lua5.1-tests",
        expected_pass: &[
            "big.lua",
            "verybig.lua",
            "api.lua",
            "attrib.lua",
            "calls.lua",
            "checktable.lua",
            "closure.lua",
            "code.lua",
            "constructs.lua",
            "db.lua",
            "errors.lua",
            "events.lua",
            "files.lua",
            "gc.lua",
            "literals.lua",
            "locals.lua",
            "main.lua",
            "math.lua",
            "nextvar.lua",
            "pm.lua",
            "sort.lua",
            "strings.lua",
            "vararg.lua",
        ],
    },
];

fn run_suite(suite: &Suite, coverage: &mut Vec<FileCoverage>) -> Vec<String> {
    let root = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(suite.dir).unwrap_or_else(|e| panic!("cd {}: {}", suite.dir, e));
    // attrib.lua's sub-package section writes `libs/P1/init.lua` and
    // `libs/P1/xuxu.lua` via `io.output(filename)`, which fails when
    // the parent dir doesn't exist (POSIX `open(O_WRONLY|O_CREAT)`
    // does not mkdir). PUC's tarball ships the directory in 5.5 but
    // not in 5.1 - 5.4, and `cargo clean` / `git clean` can leave
    // 5.5's dir empty too — both paths show up as a runtime regression
    // attributed to whichever recent change happened to land. Create
    // the dir on every run so the test always reproduces the same
    // initial filesystem state.
    let _ = std::fs::create_dir_all("libs/P1");
    let mut failures = Vec::new();
    for &name in suite.expected_pass {
        // Surface the file being attempted via stderr so a SIGSEGV
        // inside `run_file` points at the exact PUC file in the CI
        // log. Without this, a process-level crash leaves the last
        // PUC-printed line as the deceptive "last test that ran".
        eprintln!("[official_run] starting {:?}/{}", suite.version, name);
        let cov = run_file(name, suite.version);
        if let Some(err) = &cov.error {
            failures.push(format!("{:?} {name}: {err}", suite.version));
        }
        coverage.push(cov);
    }
    std::env::set_current_dir(&root).expect("restore cwd");
    failures
}

#[test]
fn official_suites_expected_pass() {
    // chdir is process-global, so all suites run sequentially inside this one
    // test. We always start from (and return to) the workspace root.
    let mut failures = Vec::new();
    let mut coverage: Vec<FileCoverage> = Vec::new();
    let mut total = 0usize;
    for suite in SUITES {
        total += suite.expected_pass.len();
        failures.extend(run_suite(suite, &mut coverage));
    }
    // Write the per-file assert-coverage report regardless of pass
    // / fail so the data is always fresh on the next inspection.
    if let Err(e) = write_coverage_report(&coverage) {
        eprintln!("coverage report write failed: {e}");
    }
    assert!(
        failures.is_empty(),
        "official suite regressions ({} of {total} files):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[path = "official_run/byte_diff.rs"]
mod byte_diff;
#[path = "official_run/report.rs"]
mod report;
#[path = "official_run/run_file.rs"]
mod run_file;

use report::write_coverage_report;
use run_file::run_file;
