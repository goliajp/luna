//! End-to-end program tests: real Lua programs run on luna AND on the
//! installed PUC reference binary, then diff stdout byte-for-byte. Any
//! semantic divergence between luna and PUC at the program level
//! surfaces here.
//!
//! Each program is run on every supported dialect 5.1-5.5. Reference
//! binaries probed (must be in PATH or at the canonical locations from
//! `tests/official_run.rs`):
//! - lua-5.1, lua-5.2, lua-5.3, lua-5.4, lua-5.5
//!
//! If a reference binary is missing for a dialect, that dialect's test
//! is **skipped** (not failed) — so CI without all binaries still
//! reports the available comparisons rather than hard-failing.
//!
//! Programs deliberately cover: pure number recursion, string
//! manipulation, table mutation, pattern matching, coroutine
//! generator, sort, error handling. See `PROGRAMS` array.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::process::{Command, Stdio};

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

// ---------------------------------------------------------------------------
// luna stdout capture: a thread_local Vec<u8> buffer that the custom
// `print` native writes into, replacing the stdout-writing default.

thread_local! {
    static CAPTURE: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn capture_print(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, luna_core::vm::LuaError> {
    // Same format as PUC print: tab-separated, trailing newline.
    // Use the Lua-level global `tostring` so the output formatting
    // matches PUC's `print` byte-for-byte (including number-to-string
    // dialect quirks).
    let tostring_key = Value::Str(vm.heap.intern(b"tostring"));
    let tostring_fn = vm.globals().get(tostring_key);
    let mut line: Vec<u8> = Vec::new();
    for i in 0..nargs {
        if i > 0 {
            line.push(b'\t');
        }
        let v = vm.nat_arg(fs, nargs, i);
        let ret = vm.call_value(tostring_fn, &[v])?;
        if let Some(Value::Str(s)) = ret.into_iter().next() {
            line.extend(s.as_bytes());
        }
    }
    line.push(b'\n');
    CAPTURE.with(|c| c.borrow_mut().extend_from_slice(&line));
    Ok(0)
}

fn drain_capture() -> Vec<u8> {
    CAPTURE.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn run_on_luna(version: LuaVersion, src: &str) -> Vec<u8> {
    drain_capture(); // reset before run
    let mut vm = Vm::new(version);
    // Override print with the capture variant.
    let f = vm.native(capture_print);
    vm.set_global("print", f).unwrap();
    // PUC's `lua -e 'src'` reports errors with chunkname `(command line)`;
    // matching that lets `tostring(err)` outputs diff cleanly.
    let cl = vm
        .load(src.as_bytes(), b"=(command line)")
        .expect("luna load");
    vm.call_value(Value::Closure(cl), &[]).expect("luna run");
    drain_capture()
}

// ---------------------------------------------------------------------------
// PUC subprocess runner: invoke the reference binary with -e and capture
// stdout. Returns Some(output) iff the binary is available.

fn reference_bin_for(version: LuaVersion) -> Option<&'static str> {
    let candidates = match version {
        LuaVersion::Lua51 => &["lua-5.1"][..],
        LuaVersion::Lua52 => &["lua-5.2"][..],
        LuaVersion::Lua53 => &["lua-5.3"][..],
        LuaVersion::Lua54 | LuaVersion::MacroLua => &["lua-5.4"][..],
        LuaVersion::Lua55 => &["lua-5.5"][..],
    };
    candidates
        .iter()
        .find(|&&c| {
            Command::new(c)
                .arg("-v")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok()
        })
        .copied()
        .map(|v| v as _)
}

fn run_on_puc(bin: &str, src: &str) -> Vec<u8> {
    let mut child = Command::new(bin)
        .arg("-e")
        .arg(src)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn PUC");
    let _ = child.stdin.take();
    let out = child.wait_with_output().expect("PUC wait");
    if !out.status.success() {
        panic!(
            "PUC {} -e failed: {}",
            bin,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    out.stdout
}

// ---------------------------------------------------------------------------
// Program catalog. Each program self-contained: can be run as
// `lua -e SOURCE`. Each MUST print its final result via `print(...)`
// so output capture works on both engines.

struct Program {
    name: &'static str,
    src: &'static str,
    // Minimum dialect that supports this program's features. Default
    // Lua51 means runs everywhere. Lua52 = `pcall` continuation-aware
    // or 3-arg `string.rep`. Lua53 = `//` `~` operators, integer
    // subtype semantics. Etc.
    min_version: LuaVersion,
}

mod basics;
mod edge_cases;
mod errors;
mod numeric_string_dialect;

const PROGRAMS: &[&[Program]] = &[
    basics::PROGRAMS,
    edge_cases::PROGRAMS,
    numeric_string_dialect::PROGRAMS,
    errors::PROGRAMS,
];

fn run_diff_for(version: LuaVersion, label: &str, prog: &Program) -> Result<(), String> {
    if prog.min_version > version {
        return Ok(()); // skip — feature not in dialect
    }

    let bin = match reference_bin_for(version) {
        Some(b) => b,
        None => return Ok(()), // reference unavailable on this host — skip
    };

    let luna_out = run_on_luna(version, prog.src);
    let puc_out = run_on_puc(bin, prog.src);

    if luna_out != puc_out {
        let luna_str = String::from_utf8_lossy(&luna_out);
        let puc_str = String::from_utf8_lossy(&puc_out);
        return Err(format!(
            "e2e divergence — [{}] program={}\n  luna stdout: {:?}\n  PUC  stdout: {:?}",
            label, prog.name, luna_str, puc_str
        ));
    }
    Ok(())
}

fn e2e_diff_for_dialect(version: LuaVersion, label: &str) {
    let mut failures: Vec<String> = Vec::new();
    for prog in PROGRAMS.iter().copied().flatten() {
        if let Err(e) = run_diff_for(version, label, prog) {
            failures.push(e);
        }
    }
    if !failures.is_empty() {
        let mut s = String::new();
        for f in &failures {
            writeln!(&mut s, "{}", f).unwrap();
        }
        panic!(
            "e2e dialect {}: {} divergences\n{}",
            label,
            failures.len(),
            s
        );
    }
}

#[test]
fn e2e_5_1() {
    e2e_diff_for_dialect(LuaVersion::Lua51, "5.1");
}
#[test]
fn e2e_5_2() {
    e2e_diff_for_dialect(LuaVersion::Lua52, "5.2");
}
#[test]
fn e2e_5_3() {
    e2e_diff_for_dialect(LuaVersion::Lua53, "5.3");
}
#[test]
fn e2e_5_4() {
    e2e_diff_for_dialect(LuaVersion::Lua54, "5.4");
}
#[test]
fn e2e_5_5() {
    e2e_diff_for_dialect(LuaVersion::Lua55, "5.5");
}
