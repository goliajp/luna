//! Running one official test file on a fresh Vm with the assert counters installed.

use super::byte_diff::{
    BYTE_DIFF_POSTAMBLE, BYTE_DIFF_PREAMBLE, byte_diff_should_skip, canonicalize_byte_diff,
    extract_byte_diff_from_puc_stdout, puc_bin_for_version, read_byte_diff_stdout,
    run_official_on_puc,
};
use super::*;

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
pub(super) const ASSERT_COUNTER_PREAMBLE: &[u8] = b"do _G.__luna_assert_total=0 _G.__luna_assert_hit=0 _G.assert=function(v,msg,...) _G.__luna_assert_total=_G.__luna_assert_total+1 if v then _G.__luna_assert_hit=_G.__luna_assert_hit+1 return v,msg,... end if msg==nil then msg='assertion failed!' end error(msg,2) end end ";

pub(super) fn run_file(name: &str, version: LuaVersion) -> FileCoverage {
    // gc.lua/gengc.lua/tracegc.lua run on Windows too: the Windows
    // weak-table crash came from two platform-independent GC
    // bugs (stale gc_top on native-call collects + weak-table tombstone
    // keys escaping clearkey), both fixed.
    //
    // cwd is the suite dir (set by the caller) so require's ./?.lua finds siblings.
    let body = match read_chunk(name, version) {
        Ok(b) => b,
        Err(error) => {
            return FileCoverage {
                version,
                file: name.to_string(),
                total: 0,
                hit: 0,
                error: Some(error),
                wrapper_skipped: false,
            };
        }
    };
    // Prepend the assert-counter preamble. Single line, so source
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
    // Opt-in byte-diff stdout capture. Prepended
    // before the assert-counter wrapper so the two wrappers are
    // independent (byte-diff redefines _G.print/_G.io.write;
    // assert-counter redefines _G.assert). Default path is
    // unchanged when the env var is absent.
    //
    // Allowlisted (version, file) pairs skip the
    // byte-diff preamble even when the env is set. See
    // `BYTE_DIFF_ALLOWLIST` for the reason per file.
    let byte_diff_enabled = std::env::var_os("LUNA_OFFICIAL_BYTE_DIFF").is_some()
        && !byte_diff_should_skip(version, name);
    let src = wrap_source(body, skip_wrapper, byte_diff_enabled);
    let label = name.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let mut vm = Vm::new(version);
            configure_vm(&mut vm, &label);
            let r = run_chunk(&mut vm, &src, &label, version);
            // Read counters back from globals. If the chunk error'd
            // before the preamble ran (e.g. compile failure) both stay at
            // 0, which is the truthful reading. Read via raw Table::get
            // so no __index metamethod can perturb the value.
            let (total, hit) = read_assert_counters(&mut vm);
            // Byte-diff comparison. Fires when env-set AND file not
            // allowlisted AND both luna + PUC actually captured a
            // buffer. Divergences are eprintln'd for triage rather
            // than failing the file (opt-in surface).
            if byte_diff_enabled && r.is_ok() {
                report_byte_diff(&mut vm, &src, &label, version);
            }
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

/// The file's bytes as PUC would load them: BOM / shebang stripped, and
/// the `_port` guard 5.1 main.lua lacks prepended.
fn read_chunk(name: &str, version: LuaVersion) -> Result<Vec<u8>, String> {
    let raw = std::fs::read(name).map_err(|e| format!("read {name}: {e}"))?;
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
    Ok(if name == "main.lua" && version == LuaVersion::Lua51 {
        let mut out = b"if _port then return end ".to_vec();
        out.extend_from_slice(stripped);
        out
    } else {
        stripped.to_vec()
    })
}

/// Prepend the assert-counter preamble and, when enabled, the byte-diff
/// capture around the body.
fn wrap_source(body: Vec<u8>, skip_wrapper: bool, byte_diff_enabled: bool) -> Vec<u8> {
    if skip_wrapper {
        body
    } else if byte_diff_enabled {
        // The byte-diff path wraps body in a local
        // function so the body's own `return X` doesn't terminate
        // the chunk before postamble runs. `assert`-counter
        // wrapper stays outside the function since it must be
        // installed globally before the body observes it.
        //
        // Note: some files reference local top-level variables
        // used later in the same chunk — those become locals of
        // `__luna_body` and are invisible to the postamble, which
        // only touches the `_G.__luna_official_stdout` global.
        const BODY_WRAP_START: &[u8] = b" local __luna_body = function() ";
        const BODY_WRAP_END: &[u8] = b" end __luna_body() ";
        let cap = ASSERT_COUNTER_PREAMBLE.len()
            + BYTE_DIFF_PREAMBLE.len()
            + BODY_WRAP_START.len()
            + body.len()
            + BODY_WRAP_END.len()
            + BYTE_DIFF_POSTAMBLE.len();
        let mut s = Vec::with_capacity(cap);
        s.extend_from_slice(BYTE_DIFF_PREAMBLE);
        s.extend_from_slice(ASSERT_COUNTER_PREAMBLE);
        s.extend_from_slice(BODY_WRAP_START);
        s.extend_from_slice(&body);
        s.extend_from_slice(BODY_WRAP_END);
        s.extend_from_slice(BYTE_DIFF_POSTAMBLE);
        s
    } else {
        let mut s = Vec::with_capacity(ASSERT_COUNTER_PREAMBLE.len() + body.len());
        s.extend_from_slice(ASSERT_COUNTER_PREAMBLE);
        s.extend_from_slice(&body);
        s
    }
}

/// Memory cap and the `_U` / `_port` / `_soft` / `_noposix` globals the
/// file runs under.
fn configure_vm(vm: &mut Vm, label: &str) {
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
    // additions to the same stress family.
    if matches!(
        label,
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
}

/// Load and call the chunk; 5.1 big.lua runs inside a coroutine driver.
fn run_chunk(vm: &mut Vm, src: &[u8], label: &str, version: LuaVersion) -> Result<(), String> {
    let chunkname = format!("@{label}");
    // PUC's outer driver (`all.lua`) wraps every chunk in
    // `coroutine.wrap(function () dofile(name) end)`, so files like
    // 5.1 big.lua that yield at top level (`function xxxx () yield()
    // end; xxxx()`) drive cleanly. luna's gate normally calls each
    // chunk directly; for the 5.1 big.lua case mirror the wrap so the
    // yield doesn't trip an "outside a coroutine" error.
    let wrap_in_coroutine = label == "big.lua" && version == LuaVersion::Lua51;
    match vm.load(src, chunkname.as_bytes()) {
        Ok(cl) => {
            let call_r = if wrap_in_coroutine {
                let driver_src = b"local f = ...; local co = coroutine.create(f); while coroutine.status(co) ~= 'dead' do local ok, err = coroutine.resume(co); if not ok then error(err) end end";
                match vm.load(driver_src, b"=driver") {
                    Ok(d) => vm.call_value(Value::Closure(d), &[Value::Closure(cl)]),
                    Err(e) => Err(luna_core::vm::error::LuaError(Value::Str(
                        vm.heap.intern(format!("driver compile: {e}").as_bytes()),
                    ))),
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
    }
}

/// Print a `[STDOUT-DIVERGE]` line when luna and PUC printed different bytes.
fn report_byte_diff(vm: &mut Vm, src: &[u8], label: &str, version: LuaVersion) {
    let Some(luna_bytes) = read_byte_diff_stdout(vm) else {
        return;
    };
    let Some(bin) = puc_bin_for_version(version) else {
        return;
    };
    match run_official_on_puc(&bin, src) {
        Some(Ok(puc_stdout)) => {
            if let Some(puc_bytes) = extract_byte_diff_from_puc_stdout(&puc_stdout) {
                let luna_canon = canonicalize_byte_diff(&luna_bytes);
                let puc_canon = canonicalize_byte_diff(&puc_bytes);
                if luna_canon != puc_canon {
                    eprintln!(
                        "[STDOUT-DIVERGE] {:?}/{}: luna_len={} puc_len={} first_diff_at={}",
                        version,
                        label,
                        luna_canon.len(),
                        puc_canon.len(),
                        luna_canon
                            .iter()
                            .zip(puc_canon.iter())
                            .position(|(a, b)| a != b)
                            .map(|i| i as isize)
                            .unwrap_or(-1)
                    );
                }
            }
        }
        Some(Err(msg)) => {
            eprintln!("[STDOUT-DIVERGE-PUC-ERR] {:?}/{}: {}", version, label, msg);
        }
        None => {} // binary missing; silent
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
pub(super) fn read_assert_counters(vm: &mut Vm) -> (i64, i64) {
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
