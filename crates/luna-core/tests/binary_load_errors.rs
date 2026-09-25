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

/// A count in luna's own dump format sizes an allocation. One the rest of
/// the chunk cannot hold is refused as a truncation; before, a corrupt
/// count asked for gigabytes and aborted the process (`fuzz_dump_reader`).
#[test]
fn a_count_larger_than_the_chunk_is_a_truncation() {
    for (version, want) in [
        (
            LuaVersion::Lua51,
            "nil bad: unexpected end in precompiled chunk",
        ),
        (LuaVersion::Lua52, "nil bad: truncated precompiled chunk"),
        (
            LuaVersion::Lua54,
            "nil bad: bad binary format (truncated chunk)",
        ),
        (
            LuaVersion::Lua55,
            "nil bad: bad binary format (truncated chunk)",
        ),
    ] {
        let mut vm = Vm::new(version);
        // the instruction count follows the chunk's source name
        let r = vm
            .eval(
                r#"
                local ld = loadstring or load
                local d = string.dump(ld("return 1", "=srcmark"))
                local at = select(2, d:find("=srcmark", 1, true))
                local bad = d:sub(1, at) .. "\255\255\255\255" .. d:sub(at + 5)
                local f, e = ld(bad, "=bad")
                return tostring(f) .. " " .. tostring(e)
                "#,
            )
            .expect("chunk runs");
        let Some(Value::Str(s)) = r.first() else {
            panic!("expected a string")
        };
        assert_eq!(String::from_utf8_lossy(s.as_bytes()), want, "{version:?}");
    }
}

/// A PUC 5.1 chunk whose one instruction is `SETUPVAL` with B = 320: 5.1's
/// B field has 9 bits, luna's has 8. The translator built the instruction
/// with an operand that did not fit, which debug builds caught as an
/// assertion failure and release builds encoded into the neighbouring
/// field. It is refused now, as a chunk luna cannot represent
/// (`fuzz_dump_reader`).
#[test]
fn a_translated_operand_that_does_not_fit_is_refused() {
    const CHUNK: [u8; 64] = [
        0x1b, 0x4c, 0x75, 0x61, 0x51, 0x00, 0x01, 0x04, 0x08, 0x04, 0x08, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0xa0, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
    ];
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.set_puc_bytecode_loading(true);
    let e = vm
        .load(&CHUNK, b"=fuzz")
        .expect_err("the chunk must be refused");
    let msg = String::from_utf8_lossy(&e.msg).into_owned();
    assert!(msg.contains("bad code in precompiled chunk"), "{msg}");
}
