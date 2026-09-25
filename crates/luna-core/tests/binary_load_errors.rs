//! A malformed binary chunk is refused with the running dialect's
//! `lundump.c` wording and chunk-name prefix (`binary string` for a chunk
//! loaded from a string under its default name, `@`/`=` stripped
//! otherwise). The expected strings are what stock PUC 5.1.5 / 5.2.4 /
//! 5.3.6 / 5.4.9 / 5.5.1 print for the same `load` calls.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const CASES: &str = r#"
local ld = loadstring or load
local function f(a, b) local sum = a + b return sum * 2, "x" .. a end
local d = string.dump(f)
local out = {}
out[1] = select(2, ld(d:sub(1, 20)))
out[2] = select(2, ld(d:sub(1, math.floor(#d / 2))))
out[3] = select(2, ld("\27Lua" .. string.rep("\0", 30)))
out[4] = select(2, ld(d:sub(1, 20), "@dir/file.luac"))
out[5] = select(2, ld(d:sub(1, 20), "=named"))
return table.concat(out, "\n")
"#;

fn messages(version: LuaVersion) -> Vec<String> {
    let mut vm = Vm::new(version);
    let v = vm.eval(CASES).expect("probe runs");
    let Value::Str(s) = v[0] else {
        panic!("probe returned {:?}", v[0]);
    };
    String::from_utf8_lossy(s.as_bytes())
        .lines()
        .map(str::to_string)
        .collect()
}

#[track_caller]
fn check(version: LuaVersion, truncated: &str, bad_header: &str) {
    let got = messages(version);
    let want = [
        format!("binary string: {truncated}"),
        format!("binary string: {truncated}"),
        format!("binary string: {bad_header}"),
        format!("dir/file.luac: {truncated}"),
        format!("named: {truncated}"),
    ];
    assert_eq!(got, want, "{version:?}");
}

#[test]
fn lua51_wording() {
    check(
        LuaVersion::Lua51,
        "unexpected end in precompiled chunk",
        "bad header in precompiled chunk",
    );
}

#[test]
fn lua52_wording() {
    check(
        LuaVersion::Lua52,
        "truncated precompiled chunk",
        "version mismatch in precompiled chunk",
    );
}

#[test]
fn lua53_wording() {
    check(
        LuaVersion::Lua53,
        "truncated precompiled chunk",
        "version mismatch in precompiled chunk",
    );
}

#[test]
fn lua54_wording() {
    check(
        LuaVersion::Lua54,
        "bad binary format (truncated chunk)",
        "bad binary format (version mismatch)",
    );
}

#[test]
fn lua55_wording() {
    check(
        LuaVersion::Lua55,
        "bad binary format (truncated chunk)",
        "bad binary format (version mismatch)",
    );
}
