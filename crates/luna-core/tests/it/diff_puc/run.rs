//! Running a fixture: on PUC as a process, on luna as lua.c runs a
//! chunk, and the outputs normalized for comparing.

use super::*;

/// Run `source` via PUC's reference binary and capture stdout.
/// Only a **missing binary** skips (dev machines without
/// `lua5.5` shouldn't fail the suite; CI installs it
/// explicitly). If PUC runs but errors (non-zero exit or
/// non-empty stderr), the fixture itself is broken and the test
/// **fails** — silently skipping here shipped fixtures that
/// never actually diffed (found in v2.12: 5 of the first 150).
pub(super) fn run_on_puc(path: &Path, bin: &str, source: &str) -> Option<String> {
    let mut child = match Command::new(bin)
        .arg("-")
        .env("TZ", "UTC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            assert!(
                !require_all(),
                "[diff_puc] cannot run PUC binary `{bin}`: {e}"
            );
            eprintln!("[diff_puc] PUC binary `{bin}` not found; skipping");
            return None;
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[diff_puc] PUC wait failed: {e}");
            return None;
        }
    };
    if !out.status.success() || !out.stderr.is_empty() {
        panic!(
            "[diff_puc] fixture {} errors on PUC itself (status={:?} stderr={}) — fix the fixture",
            path.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Run `source` as `bin -` runs it, then close it as lua.c does; output caught.
pub(super) fn run_on_luna(version: LuaVersion, bin: &str, source: &str) -> String {
    let (r, out) = panic_free("a fixture", || {
        crate::stdout_capture::caught(|| {
            let mut s = lua_c_standalone(version, bin);
            run_as_script(&mut s, source).map_err(|e| e.to_string())
        })
    }); // the Vm drops inside: what the close's finalizers print is caught
    r.expect("luna eval must not error on diff fixtures");
    String::from_utf8_lossy(&out).into_owned()
}

/// Normalize whitespace runs + trim trailing newlines so trivial
/// formatting drift (e.g. `\r\n` vs `\n`, double-newline at EOF)
/// doesn't fail the diff. Semantic content stays.
pub(super) fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}

/// v2.14 HC — error-channel normalization. Splits a Lua error
/// message into (had_position_prefix, message): a leading
/// `<chunk>:<line>: ` is stripped because chunknames differ by
/// design (PUC reads stdin → "stdin"; luna evals → "eval") and the
/// luna side's capture preamble shifts line numbers. WHETHER a
/// position prefix exists is still compared — a fixture whose error
/// is prefixed on one side and bare on the other is a real
/// divergence (see v2.13's resume_error / luaL_where fixes).
pub(super) fn normalize_err(text: &str) -> (bool, String) {
    let t = text.trim_end();
    if let Some(colon2) = t.find(": ") {
        let head = &t[..colon2];
        if let Some(colon1) = head.rfind(':') {
            let (chunk, line) = head.split_at(colon1);
            if !chunk.is_empty() && line[1..].bytes().all(|c| c.is_ascii_digit()) {
                return (true, t[colon2 + 2..].to_string());
            }
        }
    }
    (false, t.to_string())
}

/// A Vm as lua.c sets one up for `bin -`: inside `pmain`, the libraries
/// open, `arg` made (5.3 on) and, from 5.4, the collector generational.
fn lua_c_standalone(version: LuaVersion, bin: &str) -> Standalone {
    Vm::new_minimal(version)
        .into_standalone(&[bin, "-"], Some(1), StandaloneOptions::default())
        .expect("lua.c's state")
}

/// [`lua_c_standalone`]'s Vm.
pub(crate) fn lua_c_vm(version: LuaVersion, bin: &str) -> Vm {
    lua_c_standalone(version, bin).into_vm()
}

/// lua.c's `handle_script` for `-`: `source` read from stdin, run by `docall`.
fn run_as_script(s: &mut Standalone, source: &str) -> Result<(), LuaError> {
    s.script_args()?;
    let vm = s.vm_mut();
    let f = vm.load_stdin_text(source.as_bytes().to_vec())?;
    let msgh = vm.native(keep_message);
    vm.call_value_with_handler_in_c(f, &[], msgh).map(|_| ())
}

/// lua.c's `handle_script` for a file: `name` loaded by `luaL_loadfile`,
/// run by `docall`.
pub(super) fn run_file(s: &mut Standalone, name: &[u8]) -> Result<(), LuaError> {
    s.script_args()?;
    let vm = s.vm_mut();
    let f = vm.load_file(Some(name), None)?;
    let msgh = vm.native(keep_message);
    vm.call_value_with_handler_in_c(f, &[], msgh).map(|_| ())
}

/// The message handler, whose result the stdout channel never sees.
fn keep_message(_: &mut Vm, _: u32, _: u32) -> Result<u32, LuaError> {
    Ok(0)
}
