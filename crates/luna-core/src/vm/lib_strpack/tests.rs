use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::Vm;

fn run(src: &str) -> Result<Vec<Value>, String> {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let cl = vm
        .load(src.as_bytes(), b"@test")
        .map_err(|e| e.to_string())?;
    vm.call_value(Value::Closure(cl), &[])
        .map_err(|e| vm.error_text(&e))
}

#[test]
fn int_roundtrip_endianness() {
    run(r#"
        assert(string.unpack("B", string.pack("B", 0xff)) == 0xff)
        assert(string.unpack("<i4", string.pack("<i4", -1)) == -1)
        assert(string.unpack(">i4", string.pack(">i4", -1)) == -1)
        assert(string.pack("<i2", 1) == "\1\0")
        assert(string.pack(">i2", 1) == "\0\1")
        assert(string.pack("<I3", 0xAA) == "\xAA\0\0")
    "#)
    .unwrap();
}

#[test]
fn packsize_and_variable_errors() {
    run(r#"
        assert(string.packsize("i4") == 4)
        assert(string.packsize("<! c3") == 3)
        assert(string.packsize("!8 xXi8") == 8)
        local ok = pcall(string.packsize, "s")
        assert(not ok)
        local ok2 = pcall(string.packsize, "z")
        assert(not ok2)
    "#)
    .unwrap();
}

#[test]
fn strings_and_floats() {
    run(r#"
        local s = "alo"
        assert(string.unpack("z", string.pack("z", s)) == s)
        assert(string.unpack("s4", string.pack("s4", s)) == s)
        assert(string.unpack("n", string.pack("n", 1.5)) == 1.5)
        assert(string.pack("<f", 24) == string.pack(">f", 24):reverse())
        assert(string.pack("c8", "123456") == "123456\0\0")
    "#)
    .unwrap();
}

#[test]
fn overflow_and_fit_errors() {
    run(r#"
        assert(not pcall(string.pack, "<I1", -1))      -- unsigned overflow
        assert(not pcall(string.pack, ">i1", 0xFF))    -- integer overflow
        assert(not pcall(string.pack, "i0", 0))        -- out of limits
        assert(not pcall(string.pack, "i17", 0))       -- out of limits
        assert(not pcall(string.pack, "c3", "1234"))   -- longer than
        assert(not pcall(string.unpack, "i16", string.rep("\3", 16))) -- does not fit
    "#)
    .unwrap();
}
