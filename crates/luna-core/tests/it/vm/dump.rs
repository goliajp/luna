//! string.dump and the chunks it produces.

use super::*;

#[test]
fn string_dump_round_trips() {
    // basic round-trip: dump a function, reload, call
    check_int(
        "local s = string.dump(load('return 6*7')) return load(s)()",
        42,
    );
    // strip flag still produces a loadable chunk
    check_int("return load(string.dump(load('return 40+2'), true))()", 42);
    // constants of every serialisable kind survive
    check_str(
        "local f = load([[return ('a'..1)..(2.5)..tostring(true)..tostring(nil)]]) \
         return load(string.dump(f))()",
        b"a12.5truenil",
    );
    // a reloaded chunk still reaches globals through its fresh _ENV upvalue
    check_int(
        "gv = 9 local s = string.dump(load('return gv')) return load(s)()",
        9,
    );
    // nested function prototypes round-trip
    check_int(
        "local src = 'local function k() return 5 end return k()+k()' \
         return load(string.dump(load(src)))()",
        10,
    );
    // only Lua functions can be dumped
    check_error("string.dump(print)", "Lua function expected");
}

#[test]
fn string_dump_header_is_per_version() {
    // calls.lua across 5.3/5.4/5.5 byte-checks the header prefix `string.dump`
    // produces. Drive a small dump through each dialect's VM and pluck the
    // version byte (offset 4) + LUAC_INT sanity word from the right slot.
    let cases = &[
        (LuaVersion::Lua53, 0x53u8, 0x11usize),
        (LuaVersion::Lua54, 0x54u8, 0x0fusize),
        (LuaVersion::Lua55, 0x55u8, 0usize), // 5.5 splits sanity per type; just check version
    ];
    for &(version, ver_byte, luac_int_off) in cases {
        let mut vm = Vm::new(version);
        let bytes = match vm.eval("return string.dump(function () return 7 end)") {
            Ok(vs) => match vs.into_iter().next() {
                Some(Value::Str(s)) => s.as_bytes().to_vec(),
                v => panic!("{version:?}: dump returned {v:?}, expected Str"),
            },
            Err(e) => panic!("{version:?}: eval failed: {e:?}"),
        };
        assert!(
            bytes.starts_with(b"\x1bLua"),
            "{version:?}: missing signature, got {:?}",
            &bytes[..4]
        );
        assert_eq!(bytes[4], ver_byte, "{version:?}: version byte mismatch");
        if luac_int_off > 0 {
            // 5.3 / 5.4 embed LUAC_INT = 0x5678 at a fixed offset. Reading 8 le
            // bytes there guards both the layout and the value choice — flipping
            // any earlier size byte would also offset this read.
            let off = luac_int_off;
            let int_bytes: [u8; 8] = bytes[off..off + 8].try_into().unwrap();
            assert_eq!(
                i64::from_le_bytes(int_bytes),
                0x5678,
                "{version:?}: LUAC_INT mismatch at offset {off}"
            );
        }
    }
}

#[test]
fn dump_inherits_parent_source_in_child_protos() {
    // calls.lua :556 — PUC `DumpFunction` writes an empty source when a child
    // proto shares its parent's source, so a `<const>` string captured by N
    // child closures appears in the byte stream only twice (once in the
    // source text, once as the parent's constant), not 1 + 1 + N times.
    check_int(
        "local foo = load([[ \
           local str <const> = 'MARKER' \
           return { \
             function () return str end, \
             function () return str end, \
             function () return str end \
           } \
         ]]) \
         local dump = string.dump(foo) \
         local _, count = string.gsub(dump, 'MARKER', function () return 'X' end) \
         return count",
        2,
    );
}
