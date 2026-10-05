//! A float `%` compiled into an AOT trace picks the NaN the interpreter
//! picks when both operands are NaN, which follows the C `fmod` PUC is
//! built with where the binary runs (the trace calls `luna_jit_fmod`
//! from the runtime staticlib, not the C library's `fmod`).

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

use crate::host_link::host_can_link;

const BODY: &str = "
    local function num(bits) return (string.unpack('<d', string.pack('<i8', bits))) end
    local function bits(x) return (string.unpack('<i8', string.pack('<d', x))) end
    local ns = { num(0x7FF8000000000003), num(0xFFF8000000000005), num(0x7FF0000000000001),
                 num(0xFFF8000000000000), 1.5 }
    local out = {}
    for i = 1, #ns do
      for j = 1, #ns do
        local a, b, m = ns[i], ns[j]
        for _ = 1, 2000 do m = a % b end
        out[#out + 1] = string.format('%x', bits(m))
      end
    end
    local text = table.concat(out, ' ')";

fn check(stem: &str, version: LuaVersion) {
    if !host_can_link() {
        eprintln!("skipped: cargo / cc not on PATH");
        return;
    }
    let interpreted = match Vm::new(version)
        .eval(&format!("{BODY} return text"))
        .expect("the script runs")
        .first()
    {
        Some(luna_core::runtime::Value::Str(s)) => {
            String::from_utf8_lossy(s.as_bytes()).into_owned()
        }
        other => panic!("the script returned {other:?}"),
    };
    let td = tempfile::tempdir().expect("tempdir");
    let src = td.path().join(format!("{stem}.lua"));
    fs::write(&src, format!("{BODY} print(text)")).expect("write source");
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
        format!("{interpreted}\n"),
        "{version:?}: AOT and interpreter differ"
    );
}

#[test]
fn aot_fmod_nan_53() {
    check("fmodnan53", LuaVersion::Lua53);
}

#[test]
fn aot_fmod_nan_54() {
    check("fmodnan54", LuaVersion::Lua54);
}
