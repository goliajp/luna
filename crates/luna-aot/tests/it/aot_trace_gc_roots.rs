//! An AOT trace roots the tables it holds only in registers while its
//! concat runs the collector, as a JIT trace does: the same lowering builds
//! both.
//!
//! The script makes the collector due at every safe point (pause 0 with
//! eight megabytes live), then builds tables whose `name` field is a concat
//! while the table exists only in the trace's registers. Without the roots
//! the table is freed by the concat's collection and its memory reused, and
//! the check after the loop fails.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

use crate::host_link::host_can_link;

#[test]
fn aot_trace_keeps_the_table_under_construction_across_concat() {
    if !host_can_link() {
        eprintln!("skipped: cargo / cc not on PATH");
        return;
    }
    let td = tempfile::tempdir().expect("tempdir");
    let src_path = td.path().join("gc_roots.lua");
    fs::write(
        &src_path,
        "BALLAST = string.rep('x', 8 * 1024 * 1024)
collectgarbage('param', 'pause', 0)
local cycles = 0
setmetatable({}, {__gc = function() cycles = cycles + 1 end})
while cycles == 0 do local s = string.rep('y', 10000) end
local buckets = {}
for i = 1, 2000 do buckets[i] = {tokens = i, name = 'bucket' .. i} end
for i = 1, 2000 do
  local b = buckets[i]
  if type(b) ~= 'table' or b.tokens ~= i or b.name ~= 'bucket' .. i then
    error('bad ' .. i .. ' ' .. type(b) .. ' ' .. tostring(b and b.tokens))
  end
end
print('ok')
",
    )
    .expect("write source");
    let out_path = td.path().join("gc_roots_aot");
    compile_and_link(&src_path, &out_path, None, LuaVersion::Lua55)
        .unwrap_or_else(|e| panic!("compile_and_link failed: {e}"));
    let output = Command::new(&out_path)
        .env("LUNA_AOT_PROBE", "1")
        .output()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", out_path.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        (output.status.code(), stdout.as_ref()),
        (Some(0), "ok\n"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("aot_trace_fired pc="),
        "no AOT trace ran; stderr:\n{stderr}"
    );
}
