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

use std::sync::mpsc;
use std::time::Duration;

use luna::runtime::Value;
use luna::version::LuaVersion;
use luna::vm::Vm;

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
        expected_pass: &["verybig.lua",
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
        expected_pass: &["verybig.lua",
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
        expected_pass: &["verybig.lua",
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

fn run_file(name: &str, version: LuaVersion) -> Result<(), String> {
    // cwd is the suite dir (set by the caller) so require's ./?.lua finds siblings
    let raw = std::fs::read(name).map_err(|e| format!("read {name}: {e}"))?;
    // File chunks get the same BOM/shebang strip PUC's `luaL_loadfilex` applies.
    let stripped = luna::frontend::lexer::Lexer::strip_shebang_bom(&raw);
    // 5.1 main.lua never grew the `if _port then return end` sentinel that 5.2+
    // added at the top of their main.lua, so just setting `_port=true` in the
    // env doesn't short-circuit the chunk. Inject the same guard the later
    // suites self-host with — the body's first real statement is `print
    // ("testing lua.c options")` so prepending one statement is harmless;
    // the rest of the chunk (os.execute / arg / popen) is what we're
    // sidestepping anyway, and there's no portable way to honor it under the
    // gate harness.
    let src = if name == "main.lua" && version == LuaVersion::Lua51 {
        let mut out = b"if _port then return end ".to_vec();
        out.extend_from_slice(stripped);
        out
    } else {
        stripped.to_vec()
    };
    let label = name.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = Vm::new(version);
            vm.set_global("_U", Value::Bool(true));
            // attrib.lua's lines 79-356 exercise dynamic C-library loading
            // (`package.loadlib`) which luna does not ship; `_port=true` is the
            // PUC-sanctioned escape hatch for non-portable subsections.
            if label == "attrib.lua" {
                vm.set_global("_port", Value::Bool(true));
            }
            // main.lua exercises the standalone-interpreter command line
            // (`os.execute`, `arg[-N]` for the binary name, tmpfile-based
            // sub-invocations). Inside the gate harness there is no real
            // interpreter binary to dispatch back into, so set `_port=true`
            // and let the `if _port then return end` at top exit cleanly.
            if label == "main.lua" {
                vm.set_global("_port", Value::Bool(true));
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
                vm.set_global("_soft", Value::Bool(true));
            }
            // files.lua's `if not _port` block runs popen/execute/`io.tmpfile`
            // off the `arg` global (which luna does not populate from a host
            // command line) — that's the PUC-sanctioned non-portable subsection.
            // The earlier and later blocks (i/o behaviour, date/time, loadfile)
            // still run. 5.2 / 5.3 use `_noposix` (not `_port`) for the same
            // popen/`os.execute` block, so set both for cross-dialect coverage.
            if label == "files.lua" {
                vm.set_global("_port", Value::Bool(true));
                vm.set_global("_noposix", Value::Bool(true));
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
            let r = match vm.load(&src, chunkname.as_bytes()) {
                Ok(cl) => {
                    let call_r = if wrap_in_coroutine {
                        let driver_src = b"local f = ...; local co = coroutine.create(f); while coroutine.status(co) ~= 'dead' do local ok, err = coroutine.resume(co); if not ok then error(err) end end";
                        match vm.load(driver_src, b"=driver") {
                            Ok(d) => vm.call_value(Value::Closure(d), &[Value::Closure(cl)]),
                            Err(e) => Err(luna::vm::error::LuaError(
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
            let _ = tx.send(r);
        })
        .expect("spawn");
    // hard per-file timeout: a hang becomes a test failure, never a wedge
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(r) => r,
        Err(_) => Err("timed out (possible hang)".to_string()),
    }
}

fn run_suite(suite: &Suite) -> Vec<String> {
    let root = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(suite.dir)
        .unwrap_or_else(|e| panic!("cd {}: {}", suite.dir, e));
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
        if let Err(e) = run_file(name, suite.version) {
            failures.push(format!("{:?} {name}: {e}", suite.version));
        }
    }
    std::env::set_current_dir(&root).expect("restore cwd");
    failures
}

#[test]
fn official_suites_expected_pass() {
    // chdir is process-global, so all suites run sequentially inside this one
    // test. We always start from (and return to) the workspace root.
    let mut failures = Vec::new();
    let mut total = 0usize;
    for suite in SUITES {
        total += suite.expected_pass.len();
        failures.extend(run_suite(suite));
    }
    assert!(
        failures.is_empty(),
        "official suite regressions ({} of {total} files):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
