//! A binary built for a dialect other than 5.5 must run its script on a
//! `Vm` of that dialect: luna's dump header does not tell 5.1 / 5.2 / 5.5
//! apart, and a 5.5 `Vm` refuses a 5.3 / 5.4 dump as foreign PUC
//! bytecode. Each script prints `_VERSION`, uses something only its
//! dialect has, and runs a hot counted loop whose trace the binary must
//! install and dispatch.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

fn have_on_path(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success() || o.status.code().is_some())
        .unwrap_or(false)
}

const HOT_LOOP: &str = "local s = 0\nfor i = 1, 1000000 do s = s + 1 end\nprint(s)\n";

/// Build `script` for `version`, run it with the AOT probe on, and check
/// its stdout and that at least one AOT trace was installed and fired.
fn build_and_run(stem: &str, version: LuaVersion, script: &str, expected_stdout: &str) {
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src = td.path().join(format!("{stem}.lua"));
    fs::write(&src, format!("{script}{HOT_LOOP}")).expect("write source");
    let out = td.path().join(stem);
    compile_and_link(&src, &out, None, version)
        .unwrap_or_else(|e| panic!("compile_and_link {version:?} failed: {e}"));

    let output = Command::new(&out)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", out.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{version:?} binary failed (stdout: {stdout:?}, stderr: {stderr})"
    );
    assert_eq!(
        stdout,
        format!("{expected_stdout}1000000\n"),
        "{version:?} binary printed the wrong output (stderr: {stderr})"
    );

    // AOT traces are installed through a section walk Windows COFF lacks
    if cfg!(target_os = "windows") {
        return;
    }
    let installed: usize = stderr
        .lines()
        .find_map(|l| l.split("aot_trace_install_count = ").nth(1))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or_else(|| panic!("no install-count probe line; stderr:\n{stderr}"));
    assert!(
        installed >= 1,
        "{version:?}: no AOT trace installed; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "{version:?}: no AOT trace dispatched; stderr:\n{stderr}"
    );
}

#[test]
fn aot_binary_runs_lua51_script() {
    // `unpack` is a 5.1 global; `/` on integers gives a float printed as
    // "5" in 5.1 / 5.2 and "5.0" from 5.3 on
    build_and_run(
        "d51",
        LuaVersion::Lua51,
        "print(_VERSION)\nprint(unpack({1, 2, 3}))\nprint(10 / 2)\n",
        "Lua 5.1\n1\t2\t3\n5\n",
    );
}

#[test]
fn aot_binary_runs_lua52_script() {
    // `bit32` exists only in 5.2
    build_and_run(
        "d52",
        LuaVersion::Lua52,
        "print(_VERSION)\nprint(bit32.band(12, 10))\nprint(10 / 2)\n",
        "Lua 5.2\n8\n5\n",
    );
}

#[test]
fn aot_binary_runs_lua53_script() {
    // integer division and the integer subtype arrived in 5.3
    build_and_run(
        "d53",
        LuaVersion::Lua53,
        "print(_VERSION)\nprint(7 // 2, math.type(1), 10 / 2)\n",
        "Lua 5.3\n3\tinteger\t5.0\n",
    );
}

#[test]
fn aot_binary_runs_lua54_script() {
    // `<const>` locals arrived in 5.4
    build_and_run(
        "d54",
        LuaVersion::Lua54,
        "print(_VERSION)\nlocal k <const> = 6\nprint(k * 7)\n",
        "Lua 5.4\n42\n",
    );
}

#[test]
fn aot_binary_runs_macrolua_script() {
    // MacroLua's own `Vm` reports its 5.4 base; a 5.5 `Vm` cannot
    build_and_run(
        "dmacro",
        LuaVersion::MacroLua,
        "print(_VERSION)\nlocal k <const> = 6\nprint(k * 7)\n",
        "Lua 5.4\n42\n",
    );
}
