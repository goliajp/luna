//! An AOT trace keeps a table it built in an iteration correct past that
//! iteration, as a JIT trace does: the same lowering builds both. The
//! script keeps each iteration's table in a local of the enclosing scope
//! (read after the loop and by the next iteration), and leaves loops
//! early with a hash-only table live and with a table held in two
//! registers. Before the fix the AOT binary printed the tables of the
//! iteration the trace was recorded on.

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

const SCRIPT: &str = "local last = {n = 0}
for i = 1, 400 do
  local t = {n = i}
  t.prev = last
  last = t
end
local k, cur = 400, last
while cur.n ~= 0 do
  if cur.n ~= k then error('chain broken at ' .. k) end
  k, cur = k - 1, cur.prev
end
print(last.n, k)
local out, prev = {}, {n = 0}
for i = 1, 400 do
  local s = 'x' .. i
  out[i - 1] = prev
  prev = {n = i, s = s}
end
print(type(out[67]), out[67].n, prev.n)
local r
for i = 1, 400 do local t = {n = i}; if i == 350 then r = t end end
local r2
for i = 1, 400 do local t = {i}; local u = t; if i == 350 then r2 = u end end
print(r.n, r2[1])
";

#[test]
fn aot_trace_keeps_tables_carried_past_their_iteration() {
    if cfg!(target_os = "windows") {
        eprintln!("skipped: AOT trace install is not implemented on Windows COFF");
        return;
    }
    if !have_on_path("cc") || !have_on_path("cargo") {
        eprintln!("skipped: cc / cargo not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("loop_carried.lua");
    fs::write(&src_path, SCRIPT).expect("write source");
    let out_path = td.path().join("loop_carried_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua54)
        .unwrap_or_else(|e| panic!("compile_and_link failed: {e}"));
    let output = Command::new(&out_path)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", out_path.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        (output.status.code(), stdout.as_ref()),
        (Some(0), "400\t0\ntable\t67\t400\n350\t350\n"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "no AOT trace ran; stderr:\n{stderr}"
    );
}
