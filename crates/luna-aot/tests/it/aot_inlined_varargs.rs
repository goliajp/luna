//! An AOT binary whose hot loop calls a vararg function that returns two
//! values, passes them all on to another call, and calls a closure made in
//! a function the trace inlines: every callee is inlined, so its prototype
//! has a slot the deploy side fills.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

use crate::host_link::host_can_link;

/// The number on the stderr probe line `<name> = N`.
fn probe(stderr: &str, name: &str) -> usize {
    let line = stderr
        .lines()
        .find(|l| l.contains(&format!("{name} = ")))
        .unwrap_or_else(|| panic!("no {name} probe line; stderr:\n{stderr}"));
    line.rsplit(" = ")
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("could not parse {line:?}"))
}

#[test]
fn aot_binary_inlines_vararg_multi_value_calls_and_closures() {
    if !host_can_link() {
        eprintln!("skipped: cargo / cc not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("varargs.lua");
    fs::write(
        &src_path,
        br#"local function f(x, ...) local y, z = ... return x + y, z end
local function g(a, b) return a * 2 + (b or 0) end
local function make(x) return function() return x end end
local s = 0
for i = 1, 200000 do
    s = s + g(f(i, 3, 1))
    s = s + make(i)()
end
print(s)
"#,
    )
    .expect("write source");
    let out_path = td.path().join("varargs_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua54)
        .unwrap_or_else(|e| panic!("compile_and_link failed: {e}"));
    let output = Command::new(&out_path)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
    assert_eq!(stdout, "60001700000\n", "stderr:\n{stderr}");
    // f, g, make and the closure make returns
    assert!(
        probe(&stderr, "aot_proto_slots_resolved") >= 4,
        "the callees were not all inlined; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "no AOT trace ran; stderr:\n{stderr}"
    );
}
