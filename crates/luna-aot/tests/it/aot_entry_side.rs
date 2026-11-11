//! An AOT binary whose warmup saw a loop entered with two register types
//! carries the second type's trace as an entry side trace of the first and
//! attaches it at deploy: the deploy Vm enters it from the start, and the
//! output is the interpreter's, in every dialect.

use std::fs;
use std::process::Command;

use luna_aot::embed::compile_and_link;
use luna_core::version::LuaVersion;

use crate::host_link::host_can_link;

// from 5.3 on a float accumulator is refused by the integer one's trace
const FLOATS: &str = "local function f(x, n, ...) local s = x + select('#', ...)\n\
    for i = 1, n do s = s + i end return s end\n\
    print(f(0, 3000), f(0.5, 30000))\n";

// before 5.3 numbers are one type: a string compared in the loop
const STRINGS: &str = "local function f(x, n, ...) local c = select('#', ...)\n\
    for i = 1, n do if x == i then c = c + 1 end c = c + 1 end return c end\n\
    print(f(0, 3000), f('s', 30000))\n";

#[test]
fn an_aot_binary_attaches_and_enters_its_entry_side_traces() {
    if !host_can_link() {
        eprintln!("skipped: cargo / cc not on PATH");
        return;
    }
    let cases = [
        (LuaVersion::Lua51, STRINGS, "3000\t30000\n"),
        (LuaVersion::Lua52, STRINGS, "3000\t30000\n"),
        (LuaVersion::Lua53, FLOATS, "4501500\t450015000.5\n"),
        (LuaVersion::Lua54, FLOATS, "4501500\t450015000.5\n"),
        (LuaVersion::Lua55, FLOATS, "4501500\t450015000.5\n"),
    ];
    for (version, script, want) in cases {
        let td = tempfile::tempdir().expect("tempdir");
        let src = td.path().join("es.lua");
        fs::write(&src, script).expect("write source");
        let out = td.path().join("es");
        compile_and_link(&src, &out, None, version)
            .unwrap_or_else(|e| panic!("compile_and_link {version:?} failed: {e}"));
        let output = Command::new(&out)
            .env("LUNA_AOT_PROBE", "1")
            .output()
            .unwrap_or_else(|e| panic!("could not run {}: {e}", out.display()));
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(0), "{version:?}: {stderr}");
        assert_eq!(stdout, want, "{version:?}: {stderr}");
        // the root and its entry side trace, both at the loop's head
        let heads: Vec<&str> = stderr
            .lines()
            .filter_map(|l| l.split("aot_trace_installed head_pc=").nth(1))
            .collect();
        assert!(
            heads.len() >= 2
                && heads
                    .iter()
                    .any(|h| heads.iter().filter(|g| *g == h).count() == 2),
            "{version:?}: no root with its entry side trace installed; stderr:\n{stderr}"
        );
    }
}
