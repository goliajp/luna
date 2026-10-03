//! A binary built by luna-aot writes NaNs with the sign and spelling the
//! interpreter gives them, which match PUC on the same machine (the
//! `817_nan_sign` diff_puc fixture checks the interpreter against PUC).
//! The script's hot loops run as AOT traces in the binary.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

use crate::host_link::host_can_link;

const SCRIPT: &str = include_str!("../../../luna-core/tests/diff_puc/5.4/817_nan_sign.lua");

/// What the interpreter prints for the script.
fn interpreted(version: LuaVersion) -> String {
    let body = SCRIPT.replace(
        "print(table.concat(r, \"\\n\"))",
        "return table.concat(r, \"\\n\")",
    );
    assert_ne!(body, SCRIPT, "the fixture ends with its print");
    let mut vm = Vm::new(version);
    let out = vm.eval(&body).expect("the script runs");
    match out.first() {
        Some(luna_core::runtime::Value::Str(s)) => {
            format!("{}\n", String::from_utf8_lossy(s.as_bytes()))
        }
        other => panic!("the script returned {other:?}"),
    }
}

fn check(stem: &str, version: LuaVersion) {
    if !host_can_link() {
        eprintln!("skipped: cargo / cc not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src = td.path().join(format!("{stem}.lua"));
    fs::write(&src, SCRIPT).expect("write source");
    let out = td.path().join(stem);
    compile_and_link(&src, &out, None, version)
        .unwrap_or_else(|e| panic!("compile_and_link {version:?} failed: {e}"));
    let output = Command::new(&out)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", out.display()));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{version:?}: {stderr}");
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "{version:?}: no AOT trace dispatched; stderr:\n{stderr}"
    );
    let got = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    assert_eq!(
        got,
        interpreted(version),
        "{version:?}: AOT and interpreter differ"
    );
}

#[test]
fn aot_nan_sign_51() {
    check("nan51", LuaVersion::Lua51);
}

#[test]
fn aot_nan_sign_52() {
    check("nan52", LuaVersion::Lua52);
}

#[test]
fn aot_nan_sign_53() {
    check("nan53", LuaVersion::Lua53);
}

#[test]
fn aot_nan_sign_54() {
    check("nan54", LuaVersion::Lua54);
}

#[test]
fn aot_nan_sign_55() {
    check("nan55", LuaVersion::Lua55);
}
