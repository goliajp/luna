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
//! # Assert-coverage instrumentation (v2.0 Track CB-or)
//!
//! Every PUC chunk is prepended with a single-line Lua snippet that wraps
//! `_G.assert` to bump two integer counters (`__luna_assert_total`,
//! `__luna_assert_hit`). After the chunk completes (or errors) the
//! counters are read back from the Vm globals and accumulated into a
//! per-file report written to `.dev/rfcs/v2.0-cb-or-coverage-report.md`.
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

/// Lua snippet prepended to every PUC chunk. **MUST be newline-free** so
/// reported source-line numbers (used by `error("…", level)` and the
/// debug library across the test corpus) stay aligned with the original
/// PUC file.
///
/// The wrapper replicates PUC `assert` semantics directly in Lua — it
/// does **not** call the original `assert` on the failure path, because
/// doing so would attribute the error to the wrapper's source location
/// (line 1 of every chunk) instead of the caller's, which breaks tests
/// like `errors.lua` that introspect line numbers in error messages.
///
/// PUC semantics replicated:
/// - `assert(true)` / `assert(truthy, …)` returns all arguments unchanged
/// - `assert(false)` / `assert(nil)` raises `"assertion failed!"`
/// - `assert(falsy, "msg")` raises `"msg"` (string) with position prefix
///   from `error(msg, 2)` (level 2 = caller of the wrapper)
/// - `assert(falsy, errobj)` where `errobj` is non-string raises `errobj`
///   unchanged (PUC `error` skips the position prefix for non-strings;
///   so does Lua's built-in `error`)
const ASSERT_COUNTER_PREAMBLE: &[u8] = b"do _G.__luna_assert_total=0 _G.__luna_assert_hit=0 _G.assert=function(v,msg,...) _G.__luna_assert_total=_G.__luna_assert_total+1 if v then _G.__luna_assert_hit=_G.__luna_assert_hit+1 return v,msg,... end if msg==nil then msg='assertion failed!' end error(msg,2) end end ";

/// v2.16 P3.4.1 — byte-diff stdout capture preamble. Opt-in via
/// `LUNA_OFFICIAL_BYTE_DIFF=1` env var (charter §2.4 gated rollout).
/// Redirects `_G.print` and `_G.io.write` to append to a global
/// buffer `_G.__luna_official_stdout` which the harness reads back
/// after the chunk runs. Mirrors `crates/luna-core/tests/diff_puc.rs`
/// pattern.
///
/// Only applied when the env var is set — default path is
/// unchanged so existing CB-or coverage semantics are preserved.
/// v2.16 P3.4.3+ steps add PUC binary spawn per file + byte-diff
/// comparison + `[STDOUT-DIVERGE]` report tagging.
const BYTE_DIFF_PREAMBLE: &[u8] = b"do _G.__luna_official_stdout='' _G.print=function(...) local t={} local n=select('#',...) for i=1,n do t[i]=tostring(select(i,...)) end _G.__luna_official_stdout=_G.__luna_official_stdout..table.concat(t,'\\t')..'\\n' end _G.io.write=function(...) local t={} local n=select('#',...) for i=1,n do t[i]=tostring(select(i,...)) end _G.__luna_official_stdout=_G.__luna_official_stdout..table.concat(t) end end ";

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
        dir: "tests/official/lua-5.5.0-tests",
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
        dir: "tests/official/lua-5.4.8-tests",
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

fn run_file(name: &str, version: LuaVersion) -> FileCoverage {
    // v2.13 Track WUC: the v2.4-v2.12 Windows gc.lua/gengc.lua/
    // tracegc.lua CI gate is GONE. UAF-C was root-caused to two
    // platform-independent GC bugs (stale gc_top on native-call
    // collects + weak-table tombstone keys escaping clearkey),
    // fixed in `42f3b76`, and validated by 25x ASAN+gc-verify
    // Linux stress plus 5 consecutive 50-iteration Windows
    // stress runs (uafc-windows-stress.yml) on both native-heap
    // and 0xDD-poison lanes. History: .dev/known-bugs/fixed/
    // windows-gc-weak-table-uaf-c.md.
    //
    // cwd is the suite dir (set by the caller) so require's ./?.lua finds siblings.
    let raw = match std::fs::read(name) {
        Ok(b) => b,
        Err(e) => {
            return FileCoverage {
                version,
                file: name.to_string(),
                total: 0,
                hit: 0,
                error: Some(format!("read {name}: {e}")),
                wrapper_skipped: false,
            };
        }
    };
    // File chunks get the same BOM/shebang strip PUC's `luaL_loadfilex` applies.
    let stripped = luna_core::frontend::lexer::Lexer::strip_shebang_bom(&raw);
    // 5.1 main.lua never grew the `if _port then return end` sentinel that 5.2+
    // added at the top of their main.lua, so just setting `_port=true` in the
    // env doesn't short-circuit the chunk. Inject the same guard the later
    // suites self-host with — the body's first real statement is `print
    // ("testing lua.c options")` so prepending one statement is harmless;
    // the rest of the chunk (os.execute / arg / popen) is what we're
    // sidestepping anyway, and there's no portable way to honor it under the
    // gate harness.
    let body = if name == "main.lua" && version == LuaVersion::Lua51 {
        let mut out = b"if _port then return end ".to_vec();
        out.extend_from_slice(stripped);
        out
    } else {
        stripped.to_vec()
    };
    // CB-or: prepend the assert-counter preamble. Single line, so source
    // line numbers in the body remain correct. Lives at file scope so its
    // wrapper outlives every assert call in the body.
    //
    // Skip the wrapper for files that introspect `assert` / `debug`
    // behaviour in ways the wrapper cannot perfectly replicate:
    //
    //   - `errors.lua`: tests `pcall(assert)` with no arguments and
    //     checks the error message contains "value expected" (PUC's
    //     `luaL_checkany` message). The pure-Lua wrapper can't reproduce
    //     that exact phrasing without growing brittle.
    //   - `db.lua`: tests `debug.sethook("l")` against a chunk loaded
    //     without debug info, then asserts the line hook never fires.
    //     Our wrapper is a Lua function *with* debug info, so the line
    //     hook does fire on its body.
    //
    // For these files the report records `total = 0, note = "skipped"`.
    let skip_wrapper = matches!(name, "errors.lua" | "db.lua");
    // v2.16 P3.4.1 — opt-in byte-diff stdout capture. Prepended
    // before the assert-counter wrapper so the two wrappers are
    // independent (byte-diff redefines _G.print/_G.io.write;
    // assert-counter redefines _G.assert). Default path is
    // unchanged when the env var is absent.
    let byte_diff_enabled = std::env::var_os("LUNA_OFFICIAL_BYTE_DIFF").is_some();
    let src = if skip_wrapper {
        body
    } else {
        let mut cap = ASSERT_COUNTER_PREAMBLE.len() + body.len();
        if byte_diff_enabled {
            cap += BYTE_DIFF_PREAMBLE.len();
        }
        let mut s = Vec::with_capacity(cap);
        if byte_diff_enabled {
            s.extend_from_slice(BYTE_DIFF_PREAMBLE);
        }
        s.extend_from_slice(ASSERT_COUNTER_PREAMBLE);
        s.extend_from_slice(&body);
        s
    };
    let label = name.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = Vm::new(version);
            // Runtime memory cap for the four stress files PUC's outer driver
            // gates behind a host wall-clock budget. heavy.lua's `toomanyidx`
            // fills `a[i] = i` until the array part reaches `MAX_ASIZE = 1 <<
            // 27` (~134 M slots × 9 B ≈ 1.2 GB) at which point `rehash`
            // returns `TableError::Overflow`. On a 7 GB GitHub Actions ubuntu
            // runner the *peak* during the final doubling (old slab + new
            // slab + temporary `old_pairs` Vec ≈ 2.4 GB + assorted Rust /
            // cargo overhead) walked the host allocator off a cliff and
            // SIGSEGV'd before the Overflow check could fire. Arming the soft
            // cap at 1 GiB lets the run loop notice between dispatch turns,
            // run a full collect (which can't reclaim the growing `a` — it's
            // reachable), and raise a catchable `"memory cap exceeded"` Lua
            // error. heavy.lua's `pcall(function () ... end)` catches it and
            // the rest of the chunk (`print "OK"`) runs to completion. Cap
            // is fire-once + disarms after firing, so the post-pcall tail
            // sees no further pressure. For verybig/memerr/sort the cap is
            // pure headroom — none of them push net live bytes anywhere near
            // 1 GiB (verybig has `_soft=true` set below, memerr early-returns
            // when `T` is nil, sort's working set is ~50k Values ≈ 1.2 MB) —
            // but pinning it here is defense-in-depth against future
            // additions to the same stress family. Tracked under
            // `.dev/known-bugs/fixed/heavy-lua-sigsegv-under-128mb-loadrep.md`.
            if matches!(
                label.as_str(),
                "heavy.lua" | "verybig.lua" | "memerr.lua" | "sort.lua"
            ) {
                vm.set_memory_cap(Some(1usize << 30));
            }
            vm.set_global("_U", Value::Bool(true)).unwrap();
            // attrib.lua's lines 79-356 exercise dynamic C-library loading
            // (`package.loadlib`) which luna does not ship; `_port=true` is the
            // PUC-sanctioned escape hatch for non-portable subsections.
            if label == "attrib.lua" {
                vm.set_global("_port", Value::Bool(true)).unwrap();
            }
            // main.lua exercises the standalone-interpreter command line
            // (`os.execute`, `arg[-N]` for the binary name, tmpfile-based
            // sub-invocations). Inside the gate harness there is no real
            // interpreter binary to dispatch back into, so set `_port=true`
            // and let the `if _port then return end` at top exit cleanly.
            if label == "main.lua" {
                vm.set_global("_port", Value::Bool(true)).unwrap();
            }
            // big.lua / verybig.lua's `if _soft then return … end` short-circuits
            // the multi-megabyte-prog / 70k-line-prog generation that PUC's
            // outer driver (all.lua) gates behind a wall-clock budget. The gate
            // harness honours the same escape hatch: still verifies the early
            // assertions (table-construction round-trip, RK boundary cases) but
            // skips the synthesized-program section that depends on either a
            // top-level `coroutine.yield` driver (big.lua) or platform-tunable
            // limits (verybig.lua).
            if label == "big.lua" || label == "verybig.lua" {
                vm.set_global("_soft", Value::Bool(true)).unwrap();
            }
            // files.lua's `if not _port` block runs popen/execute/`io.tmpfile`
            // off the `arg` global (which luna does not populate from a host
            // command line) — that's the PUC-sanctioned non-portable subsection.
            // The earlier and later blocks (i/o behaviour, date/time, loadfile)
            // still run. 5.2 / 5.3 use `_noposix` (not `_port`) for the same
            // popen/`os.execute` block, so set both for cross-dialect coverage.
            if label == "files.lua" {
                vm.set_global("_port", Value::Bool(true)).unwrap();
                vm.set_global("_noposix", Value::Bool(true)).unwrap();
            }
            let chunkname = format!("@{label}");
            // PUC's outer driver (`all.lua`) wraps every chunk in
            // `coroutine.wrap(function () dofile(name) end)`, so files like
            // 5.1 big.lua that yield at top level (`function xxxx () yield()
            // end; xxxx()`) drive cleanly. luna's gate normally calls each
            // chunk directly; for the 5.1 big.lua case mirror the wrap so the
            // yield doesn't trip an "outside a coroutine" error.
            let wrap_in_coroutine =
                label == "big.lua" && version == LuaVersion::Lua51;
            let r: Result<(), String> = match vm.load(&src, chunkname.as_bytes()) {
                Ok(cl) => {
                    let call_r = if wrap_in_coroutine {
                        let driver_src = b"local f = ...; local co = coroutine.create(f); while coroutine.status(co) ~= 'dead' do local ok, err = coroutine.resume(co); if not ok then error(err) end end";
                        match vm.load(driver_src, b"=driver") {
                            Ok(d) => vm.call_value(Value::Closure(d), &[Value::Closure(cl)]),
                            Err(e) => Err(luna_core::vm::error::LuaError(
                                Value::Str(vm.heap.intern(format!("driver compile: {e}").as_bytes())),
                            )),
                        }
                    } else {
                        vm.call_value(Value::Closure(cl), &[])
                    };
                    match call_r {
                        Ok(_) => Ok(()),
                        Err(e) => Err(format!("runtime: {:.200}", vm.error_text(&e))),
                    }
                }
                Err(e) => Err(format!("compile: {e}")),
            };
            // CB-or: read counters back from globals. If the chunk error'd
            // before the preamble ran (e.g. compile failure) both stay at
            // 0, which is the truthful reading. Read via raw Table::get
            // so no __index metamethod can perturb the value.
            let (total, hit) = read_assert_counters(&mut vm);
            let _ = tx.send((r, total, hit));
        })
        .expect("spawn");
    // Hard per-file timeout: a hang becomes a test failure, never a wedge.
    //
    // `heavy.lua` pcalls a `t[i] = i` loop that grows the array part up to
    // `table::MAX_ASIZE = 1 << 27` (134M entries × 9 bytes ≈ 1.2 GB) before
    // `rehash` returns `TableError::Overflow`. Each grow is O(N), so the
    // total work is dominated by the final 67M → 134M doubling. Measured
    // debug-build runtime is 105-115s on macOS arm64; release builds
    // finish the same file in ~8s. We give `heavy.lua` a 180s budget so
    // the debug-build gate doesn't false-positive on the intentional
    // stress test (the only thing that ever takes >60s here). Every other
    // file keeps the original 60s; anything that genuinely hangs still
    // trips the budget.
    let budget = if name == "heavy.lua" {
        Duration::from_secs(180)
    } else {
        Duration::from_secs(60)
    };
    match rx.recv_timeout(budget) {
        Ok((r, total, hit)) => FileCoverage {
            version,
            file: name.to_string(),
            total,
            hit,
            error: r.err(),
            wrapper_skipped: skip_wrapper,
        },
        Err(_) => FileCoverage {
            version,
            file: name.to_string(),
            total: 0,
            hit: 0,
            error: Some("timed out (possible hang)".to_string()),
            wrapper_skipped: skip_wrapper,
        },
    }
}

/// Read `__luna_assert_total` / `__luna_assert_hit` out of the Vm globals
/// table. Returns `(0, 0)` when either key is missing or non-integer
/// (e.g. the preamble never ran because the chunk failed to compile).
///
/// Takes `&mut Vm` so it can `heap.intern` the key string for the lookup.
/// Interning a never-before-seen key is harmless — it adds one short
/// string to the intern table and returns a fresh `Gc<LuaStr>`; the
/// subsequent `globals.get` simply returns `Value::Nil` for that key.
fn read_assert_counters(vm: &mut Vm) -> (i64, i64) {
    fn get_i64(vm: &mut Vm, key: &str) -> i64 {
        let k = Value::Str(vm.heap.intern(key.as_bytes()));
        let globals = vm.globals();
        match globals.get(k) {
            Value::Int(i) => i,
            Value::Float(f) => f as i64,
            _ => 0,
        }
    }
    (
        get_i64(vm, "__luna_assert_total"),
        get_i64(vm, "__luna_assert_hit"),
    )
}

/// v2.16 P3.4.2 — read the byte-diff stdout capture buffer set by
/// `BYTE_DIFF_PREAMBLE`. Returns `None` when the global is absent
/// (the env var was off, or the chunk errored before the preamble
/// installed the buffer). Bytes come out of the Lua string
/// unchanged — no re-encoding.
///
/// Not yet wired into the run loop — that lands in P3.4.4 alongside
/// the PUC binary spawn (P3.4.3). Allow the dead-code warning until
/// then; CI's `-D warnings` would otherwise trip.
#[allow(dead_code)]
fn read_byte_diff_stdout(vm: &mut Vm) -> Option<Vec<u8>> {
    let k = Value::Str(vm.heap.intern(b"__luna_official_stdout"));
    let globals = vm.globals();
    match globals.get(k) {
        Value::Str(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

/// v2.16 P3.4.3 — resolve the per-dialect PUC interpreter path.
/// Mirrors `crates/luna-core/tests/diff_puc.rs::puc_bin_for`.
/// Returns `None` when the env var is unset for a non-5.5 dialect;
/// 5.5 falls back to `PUC_LUA` env then bare `lua5.5` in PATH.
#[allow(dead_code)]
fn puc_bin_for_version(version: LuaVersion) -> Option<String> {
    let env_key = match version {
        LuaVersion::Lua51 => "PUC_LUA_51",
        LuaVersion::Lua52 => "PUC_LUA_52",
        LuaVersion::Lua53 => "PUC_LUA_53",
        LuaVersion::Lua54 => "PUC_LUA_54",
        LuaVersion::Lua55 => "PUC_LUA_55",
        // MacroLua is a compat variant that inherits 5.4 semantics
        // (per version.rs comment); byte-diff against PUC 5.4.
        LuaVersion::MacroLua => "PUC_LUA_54",
    };
    if let Ok(b) = std::env::var(env_key) {
        return Some(b);
    }
    if matches!(version, LuaVersion::Lua55) {
        return Some(std::env::var("PUC_LUA").unwrap_or_else(|_| "lua5.5".to_string()));
    }
    None
}

/// v2.16 P3.4.3 — spawn PUC on the given source file and capture
/// stdout as raw bytes. `source` is passed via stdin (matching
/// diff_puc.rs's `-` invocation). Returns `None` when the binary is
/// missing (dev-machine friendliness). PUC-side errors (non-zero
/// exit / stderr) surface as `Err` so the harness can decide whether
/// to allowlist or fail — different files legitimately error at the
/// PUC layer (e.g. `attrib.lua` when the sub-package section can't
/// write `libs/P1/`).
///
/// Bytes come back unchanged — canonicalization (source-path
/// normalization, hex-address scrub) is a separate pass in P3.4.5.
#[allow(dead_code)]
fn run_official_on_puc(bin: &str, source: &[u8]) -> Option<Result<Vec<u8>, String>> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = match Command::new(bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return None, // binary missing
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source);
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Some(Err(format!("PUC wait failed: {e}"))),
    };
    if !out.status.success() {
        return Some(Err(format!(
            "PUC non-zero exit (status={:?} stderr={})",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Some(Ok(out.stdout))
}

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
    // CB-or: write the per-file assert-coverage report regardless of pass
    // / fail so the data is always fresh on the next inspection.
    if let Err(e) = write_coverage_report(&coverage) {
        eprintln!("CB-or: coverage report write failed: {e}");
    }
    assert!(
        failures.is_empty(),
        "official suite regressions ({} of {total} files):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Write `.dev/rfcs/v2.0-cb-or-coverage-report.md` (resolved against the
/// luna-core manifest dir, i.e. workspace-root + `.dev/rfcs/…`).
///
/// Emits one Markdown table with one row per file (sorted by hit-rate
/// ascending so low-coverage files surface at the top) plus aggregate
/// totals. Files below 80% hit-rate get a `[WARN]` tag and a stderr
/// notice with a hint that a `_port` / `_soft` / `_noposix` gate is
/// likely skipping body.
fn write_coverage_report(coverage: &[FileCoverage]) -> std::io::Result<()> {
    // CARGO_MANIFEST_DIR for luna-core is `<workspace>/crates/luna-core`,
    // so the report lands at `<workspace>/.dev/rfcs/…` regardless of
    // whether we run from the main checkout or a worktree.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR has at least 2 ancestors");
    let dest_dir = workspace_root.join(".dev").join("rfcs");
    std::fs::create_dir_all(&dest_dir)?;
    let dest = dest_dir.join("v2.0-cb-or-coverage-report.md");

    // Aggregate. `wrapper_skipped` files are counted separately so the
    // ge80/lt80 buckets reflect only files where the wrapper actually
    // ran.
    let total_files = coverage.len();
    let mut total_asserts: i64 = 0;
    let mut total_hits: i64 = 0;
    let mut ge80 = 0usize;
    let mut lt80 = 0usize;
    let mut zero_total = 0usize;
    let mut skipped = 0usize;
    for c in coverage {
        total_asserts += c.total;
        total_hits += c.hit;
        if c.wrapper_skipped {
            skipped += 1;
        } else if c.total == 0 {
            zero_total += 1;
            lt80 += 1;
        } else {
            let rate = (c.hit as f64) / (c.total as f64);
            if rate >= 0.80 {
                ge80 += 1;
            } else {
                lt80 += 1;
            }
        }
    }

    // Sort by hit-rate ascending; files with total==0 sort first (rate
    // treated as -1 for sort purposes so they bubble up).
    let mut rows: Vec<&FileCoverage> = coverage.iter().collect();
    rows.sort_by(|a, b| {
        let ra = if a.total == 0 {
            -1.0
        } else {
            (a.hit as f64) / (a.total as f64)
        };
        let rb = if b.total == 0 {
            -1.0
        } else {
            (b.hit as f64) / (b.total as f64)
        };
        ra.partial_cmp(&rb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.file.cmp(&b.file))
    });

    let mut out = String::new();
    out.push_str("# v2.0 CB-or — assert-coverage report\n\n");
    out.push_str("Auto-generated by `crates/luna-core/tests/official_run.rs`.\n");
    out.push_str("Re-generate with `cargo test -p luna-core --test official_run`.\n\n");
    out.push_str("## Methodology\n\n");
    out.push_str("Each PUC chunk is prepended with a single-line Lua snippet that\n");
    out.push_str("wraps `_G.assert` to bump `__luna_assert_total` on every call and\n");
    out.push_str("`__luna_assert_hit` when the first argument is truthy. After the\n");
    out.push_str("chunk completes (or errors) the counters are read back from the\n");
    out.push_str("Vm globals.\n\n");
    out.push_str("- `total` — number of `assert(...)` calls actually reached\n");
    out.push_str("- `hit`   — of those, how many had a truthy first argument\n");
    out.push_str("- `rate`  — `hit / total` (100% means every reached assert\n");
    out.push_str("  passed; `< 100%` would indicate a failed-but-pcall'd assert)\n\n");
    out.push_str("**Note**: `rate` is *not* coverage relative to in-source\n");
    out.push_str("`assert(...)` calls — it is **reached / executed** asserts only.\n");
    out.push_str("Files with very low `total` relative to in-source counts are the\n");
    out.push_str("ones to investigate (a `_port` / `_soft` / `_noposix` gate is\n");
    out.push_str("likely skipping a big block of asserts).\n\n");
    out.push_str("## Aggregate\n\n");
    out.push_str(&format!("- Total PUC files exercised: **{total_files}**\n"));
    out.push_str(&format!("- Total asserts reached: **{total_asserts}**\n"));
    out.push_str(&format!("- Total asserts passing: **{total_hits}**\n"));
    out.push_str(&format!(
        "- Wrapper-skipped files (assert/debug introspection): **{skipped}**\n"
    ));
    out.push_str(&format!(
        "- Of instrumented files, `rate >= 80%`: **{ge80}**\n"
    ));
    out.push_str(&format!(
        "- Of instrumented files, `rate <  80%`: **{lt80}** (of which `total == 0`: {zero_total})\n\n"
    ));
    out.push_str("## Per-file (sorted by rate ascending)\n\n");
    out.push_str("| version | file | total | hit | rate | status |\n");
    out.push_str("|---|---|---:|---:|---:|---|\n");
    for c in &rows {
        let rate_str = if c.total == 0 {
            "—".to_string()
        } else {
            format!("{:.1}%", (c.hit as f64) / (c.total as f64) * 100.0)
        };
        let status = if c.wrapper_skipped {
            "SKIPPED: wrapper would break assert/debug introspection".to_string()
        } else if let Some(err) = &c.error {
            format!("ERROR: {}", err.chars().take(80).collect::<String>())
        } else if c.total == 0 {
            "WARN: 0 asserts reached".to_string()
        } else if (c.hit as f64) / (c.total as f64) < 0.80 {
            "WARN: < 80% hit-rate".to_string()
        } else {
            "ok".to_string()
        };
        out.push_str(&format!(
            "| {:?} | `{}` | {} | {} | {} | {} |\n",
            c.version, c.file, c.total, c.hit, rate_str, status
        ));
    }
    out.push_str("\n## Skip-gate hints\n\n");
    out.push_str("Files that consistently report a low `total` across dialects are\n");
    out.push_str("guarded by one of the harness gates set in `run_file`:\n\n");
    out.push_str("- `attrib.lua` — `_port=true` (skips `package.loadlib` C-library\n");
    out.push_str("  block, lines ~79-356)\n");
    out.push_str("- `main.lua`   — `_port=true` (skips `os.execute` / `arg` /\n");
    out.push_str("  popen sub-invocation block; 5.1 also has a top-level early\n");
    out.push_str("  return shim)\n");
    out.push_str("- `big.lua`, `verybig.lua` — `_soft=true` (skips multi-MB\n");
    out.push_str("  synthesized-program section)\n");
    out.push_str("- `files.lua` — `_port=true` + `_noposix=true` (skips\n");
    out.push_str("  popen/execute/`io.tmpfile`-via-`arg` block)\n\n");
    out.push_str("Counter-clobber risk: the harness wrapper sits at file scope; if\n");
    out.push_str("a PUC file later rebinds `_G.assert` itself, asserts that run\n");
    out.push_str("through the rebound `_G.assert` while the original is shadowed\n");
    out.push_str("go uncounted. A grep of `tests/official/` for `_G.assert =`\n");
    out.push_str("returned no matches, so this is not believed to affect any file\n");
    out.push_str("in the current suite.\n\n");
    out.push_str("## Wrapper-skip list\n\n");
    out.push_str("Two test files run **without** the assert-counter wrapper because\n");
    out.push_str("they introspect `assert` / `debug` behaviour the wrapper cannot\n");
    out.push_str("faithfully replicate:\n\n");
    out.push_str("- `errors.lua` — `pcall(assert)` (zero args) checks the error\n");
    out.push_str("  message contains `\"value expected\"`, which is the C-side\n");
    out.push_str("  `luaL_checkany` phrasing. The pure-Lua wrapper would emit\n");
    out.push_str("  `\"assertion failed!\"` instead.\n");
    out.push_str("- `db.lua`  — installs a `debug.sethook(\"l\")` line hook against\n");
    out.push_str("  a chunk loaded without debug info and asserts the hook never\n");
    out.push_str("  fires. The wrapper is a Lua function *with* debug info, so\n");
    out.push_str("  the line hook would fire on its body.\n\n");
    out.push_str("These two are reported with `total = 0` and status `SKIPPED:`\n");
    out.push_str("in the per-file table. They still run end-to-end under the\n");
    out.push_str("normal gate, just without coverage instrumentation.\n");

    std::fs::write(&dest, out)?;
    Ok(())
}
