//! An AOT binary whose hot loop calls methods of another function: the
//! trace inlines them, and checks each callee's prototype against a slot
//! the deploy side fills with the loaded chunk's prototype of the same
//! stable hash.

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
fn aot_binary_runs_a_loop_with_inlined_method_calls() {
    if cfg!(target_os = "windows") {
        eprintln!("skipped: AOT trace install is not wired on Windows COFF");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("methods.lua");
    fs::write(
        &src_path,
        br#"local cls = {}
cls.__index = cls
function cls:get(k) return self.t[k] end
function cls:set(k, v) self.t[k] = v end
function cls:incr(k, by)
    self.t[k] = (self.t[k] or 0) + by
    return self.t[k]
end
local o = setmetatable({t = {}}, cls)
local last = 0
for i = 1, 200000 do
    o:set("k", i)
    local v = o:get("k")
    last = o:incr("k", 1) + v
end
print(last)
"#,
    )
    .expect("write source");
    let out_path = td.path().join("methods_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55)
        .unwrap_or_else(|e| panic!("compile_and_link failed: {e}"));
    let output = Command::new(&out_path)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "stderr:\n{stderr}");
    assert_eq!(stdout, "400001\n", "stderr:\n{stderr}");
    assert!(
        probe(&stderr, "aot_proto_slots_resolved") >= 3,
        "the three methods' proto slots were not filled; stderr:\n{stderr}"
    );
    assert!(
        probe(&stderr, "aot_trace_install_count") >= 1,
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "no AOT trace ran; stderr:\n{stderr}"
    );
}
