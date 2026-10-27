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
        // luna's own format (`string.dump` writes PUC's), where the
        // instruction count follows the chunk's source name
        let f = vm.load(b"return 1", b"=srcmark").expect("compiles");
        let luna_chunk = luna_core::vm::dump::dump(&f.proto, false, version);
        let literal: String = luna_chunk.iter().map(|b| format!("\\{b}")).collect();
        let r = vm
            .eval(&format!(
                r#"
                local ld = loadstring or load
                local d = "{literal}"
                local at = select(2, d:find("=srcmark", 1, true))
                local bad = d:sub(1, at) .. "\255\255\255\255" .. d:sub(at + 5)
                local f, e = ld(bad, "=bad")
                return tostring(f) .. " " .. tostring(e)
                "#
            ))
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
    // the header with the target's `size_t`, then the source's length in it
    const SIZE_T: usize = std::mem::size_of::<usize>();
    const HEADER: [u8; 12] = [
        0x1b,
        0x4c,
        0x75,
        0x61,
        0x51,
        0x00,
        0x01,
        0x04,
        SIZE_T as u8,
        0x04,
        0x08,
        0x00,
    ];
    const REST: [u8; 44] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    let chunk = [&HEADER[..], &4usize.to_le_bytes(), &REST].concat();
    let mut vm = Vm::new(LuaVersion::Lua51);
    vm.set_puc_bytecode_loading(true);
    let e = vm
        .load(&chunk, b"=fuzz")
        .expect_err("the chunk must be refused");
    let msg = String::from_utf8_lossy(&e.msg).into_owned();
    assert!(msg.contains("bad code in precompiled chunk"), "{msg}");
}

/// A 5.4 chunk whose source is a long string of `n` bytes, cut after
/// three. PUC makes the string before it reads the bytes: the count holds
/// it until the next collection, and a size its allocator refuses is the
/// memory error. Each line: the size, the load's results, the count's
/// growth in KB after the load and after a collection, as PUC 5.4.9 gives
/// them on 64-bit Linux.
#[test]
fn a_cut_long_string_is_made_before_its_bytes_are_read() {
    const SCRIPT: &str = r#"
        local function varint(x)
          local t = {string.char(0x80 | (x % 128))}
          x = x // 128
          while x > 0 do table.insert(t, 1, string.char(x % 128)); x = x // 128 end
          return table.concat(t)
        end
        local hdr = string.dump(function() return 1 end):sub(1, 31)
        local out = {}
        for _, n in ipairs(SIZES) do
          local bad = hdr .. "\1" .. varint(n + 1) .. "abc"
          collectgarbage(); local c0 = collectgarbage("count")
          local f, e = load(bad, "=bad")
          local c1 = collectgarbage("count")
          collectgarbage(); local c2 = collectgarbage("count")
          out[#out + 1] = table.concat({n, tostring(f), e, math.floor(c1 - c0), math.floor(c2 - c0)}, " ")
        end
        return table.concat(out, "\n")
    "#;
    let mut cases = vec![(
        600u64 << 20,
        "629145600 nil bad: bad binary format (truncated chunk) 614400 0",
    )];
    // past any address space; a 32-bit size_t cannot hold it at all
    #[cfg(target_pointer_width = "64")]
    cases.push((1u64 << 60, "1152921504606846976 nil not enough memory 0 0"));
    let sizes: Vec<String> = cases.iter().map(|c| c.0.to_string()).collect();
    let want: Vec<&str> = cases.iter().map(|c| c.1).collect();
    let mut vm = Vm::new(LuaVersion::Lua54);
    let src = SCRIPT.replace("SIZES", &format!("{{{}}}", sizes.join(", ")));
    let got = match vm.eval(&src).expect("script").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("a listing, got {other:?}"),
    };
    assert_eq!(got, want.join("\n"));
}

/// A 5.1 or 5.2 chunk whose first function's code count is -1: PUC's
/// `LoadInt` refuses it before anything is allocated for the code. The
/// count's growth in bytes is what the load made before, and the message,
/// as PUC 5.1.5 and 5.2.4 give it.
#[test]
fn a_negative_count_is_refused_before_its_array_is_made() {
    const SCRIPT: &str = r#"
        local ld = loadstring or load
        local d = string.dump(function() end)
        local bad
        if _VERSION == "Lua 5.1" then
          -- the header, no source, the lines, four bytes, the count
          bad = d:sub(1, 12) .. ("\0"):rep(20) .. "\255\255\255\255"
        else
          bad = d:sub(1, 18) .. ("\0"):rep(11) .. "\255\255\255\255"
        end
        collectgarbage(); local c0 = collectgarbage("count")
        local f, e = ld(bad, "=bad")
        return tostring(f) .. " " .. e .. " " .. (collectgarbage("count") - c0) * 1024
    "#;
    // 64-bit and 32-bit (`gcc -m32`) builds of PUC
    let (b51, b52) = if cfg!(target_pointer_width = "64") {
        (344, 386)
    } else {
        (252, 278)
    };
    for (version, want) in [
        (
            LuaVersion::Lua51,
            format!("nil bad: bad integer in precompiled chunk {b51}"),
        ),
        (
            LuaVersion::Lua52,
            format!("nil bad: corrupted precompiled chunk {b52}"),
        ),
    ] {
        let mut vm = Vm::new(version);
        let got = match vm.eval(SCRIPT).expect("script").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("a result, got {other:?}"),
        };
        assert_eq!(got, want, "{version:?}");
    }
}

/// A chunk whose code count is too big for its array: on a 32-bit target
/// the array's bytes do not fit a `size_t`, and PUC raises
/// `luaM_toobig`'s runtime error, which the message handler running then
/// gets; a negative 5.3 count asks the allocator for nearly all of
/// memory, the memory error. As 32-bit and 64-bit builds of PUC give it,
/// under `pcall` and under `xpcall` with a handler.
#[test]
fn a_code_array_too_big_for_size_t_is_refused_as_puc_refuses_it() {
    const SCRIPT: &str = include_str!("binary_load_errors/code_count.lua");
    let run = |v: LuaVersion, n: u64| {
        let mut vm = Vm::new(v);
        let src = SCRIPT.replace("local n = N", &format!("local n = {n}"));
        match vm.eval(&src).expect("script").first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("{v:?}: {other:?}"),
        }
    };
    let oom = "true nil not enough memory|true nil not enough memory";
    assert_eq!(run(LuaVersion::Lua53, u64::from(u32::MAX)), oom);
    if cfg!(target_pointer_width = "32") {
        let too_big = "true nil memory allocation error: block too big|\
                       true nil H(memory allocation error: block too big)";
        for v in [
            LuaVersion::Lua51,
            LuaVersion::Lua52,
            LuaVersion::Lua53,
            LuaVersion::Lua54,
            LuaVersion::Lua55,
        ] {
            assert_eq!(run(v, 1 << 30), too_big, "{v:?}");
        }
    }
}
