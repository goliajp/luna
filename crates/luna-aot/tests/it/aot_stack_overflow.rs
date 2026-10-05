//! An AOT binary meets unbounded nesting the way PUC does: the error its
//! C-call or Lua-stack limit raises, caught by pcall, never a crash, in
//! every dialect, with the binary's compiled traces installed and running.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

use crate::host_link::host_can_link;

const SCRIPT: &str = r#"local function norm(e) return ((tostring(e):gsub("^.*:%d+: ", "@ "))) end
local t t = setmetatable({}, {__index = function(t, k) return t[k] end})
print(norm(select(2, pcall(function() return t.x end))))
local function f() return f() + 1 end
print(norm(select(2, pcall(f))))
local function g() return (string.gsub("x", "x", function() return g() end)) end
print(norm(select(2, pcall(g))))
local function d(n) if n == 0 then return 0 end return 1 + d(n - 1) end
local s = 0
for i = 1, 2000 do s = s + d(20) end
print(s)
print(norm(select(2, pcall(d, -1))))
local n = 0
for i = 1, 1000000 do n = n + 1 end
print(n)
"#;

const EXPECTED: &str =
    "@ C stack overflow\n@ stack overflow\nC stack overflow\n40000\n@ stack overflow\n1000000\n";

fn run(stem: &str, version: LuaVersion) {
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
    let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{version:?} binary failed (stdout: {stdout:?}, stderr: {stderr})"
    );
    assert_eq!(stdout, EXPECTED, "{version:?} (stderr: {stderr})");
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "{version:?}: no AOT trace dispatched; stderr:\n{stderr}"
    );
}

#[test]
fn aot_stack_overflow_51() {
    run("so51", LuaVersion::Lua51);
}

#[test]
fn aot_stack_overflow_52() {
    run("so52", LuaVersion::Lua52);
}

#[test]
fn aot_stack_overflow_53() {
    run("so53", LuaVersion::Lua53);
}

#[test]
fn aot_stack_overflow_54() {
    run("so54", LuaVersion::Lua54);
}

#[test]
fn aot_stack_overflow_55() {
    run("so55", LuaVersion::Lua55);
}
