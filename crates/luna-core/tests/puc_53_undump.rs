//! PUC 5.3 chunk loading on hand-made chunks: header diagnostics
//! (`LUAC_INT`, `LUAC_NUM`, the `sizeof` bytes), the gate, constant tags
//! and `LOADBOOL` with a skip.
//!
//! What a real `luac 5.3` chunk does in luna is pinned by
//! `diff_puc.rs::diff_puc_bytecode`, which runs the whole diff_puc corpus
//! through stock `luac` (`PUC_LUAC_53`) and compares with PUC.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// PUC 5.3 binary header — 33 bytes. Mirrors `lundump.h` 5.3.6 +
/// `lua-5.3-src/lua-5.3.6/src/lundump.c::checkHeader`.
const HEADER_53: [u8; 33] = [
    0x1b, b'L', b'u', b'a', 0x53, 0x00, // signature + version + format
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n', // LUAC_DATA
    4,     // sizeof(int)
    8,     // sizeof(size_t)
    4,     // sizeof(Instruction)
    8,     // sizeof(lua_Integer)
    8,     // sizeof(lua_Number)
    // LUAC_INT = 0x5678 (LE i64)
    0x78, 0x56, 0, 0, 0, 0, 0, 0, // LUAC_NUM = 370.5 (LE f64)
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40,
];

/// Encode a 5.3 iABC instruction in `op:6 | A:8 | C:9 | B:9` layout.
fn enc(op: u8, a: u32, b: u32, c: u32) -> u32 {
    debug_assert!(op < 64);
    debug_assert!(a < 256);
    debug_assert!(b < 512);
    debug_assert!(c < 512);
    (op as u32) | (a << 6) | (c << 14) | (b << 23)
}

fn enc_bx(op: u8, a: u32, bx: u32) -> u32 {
    debug_assert!(bx < (1 << 18));
    (op as u32) | (a << 6) | (bx << 14)
}

#[allow(dead_code)]
fn enc_sbx(op: u8, a: u32, sbx: i32) -> u32 {
    let bx = (sbx + 131071) as u32;
    enc_bx(op, a, bx)
}

/// Code a constant-pool index as a PUC 5.3 RK operand. `BITRK = 1 << 8`.
#[allow(dead_code)]
fn rk(idx: u32) -> u32 {
    debug_assert!(idx < 256);
    idx | (1 << 8)
}

/// PUC 5.3 length-prefixed string. byte = (len+1) if < 0xFF, else
/// 0xFF followed by a size_t (8 LE bytes) holding (len+1). Empty
/// strings ship as a single `0x00` byte (size==0 → NULL sentinel).
fn puc53_str(s: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + s.len() + 8);
    if s.is_empty() {
        v.push(0);
        return v;
    }
    let sized = (s.len() as u64) + 1;
    if sized < 0xFF {
        v.push(sized as u8);
    } else {
        v.push(0xFF);
        v.extend_from_slice(&sized.to_le_bytes());
    }
    v.extend_from_slice(s);
    v
}

fn put_i32(out: &mut Vec<u8>, v: i32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_f64(out: &mut Vec<u8>, v: f64) {
    out.extend_from_slice(&v.to_bits().to_le_bytes());
}

// ---- opcode constants — kept in sync with translator's table ----
const OP_LOADK: u8 = 1;
const OP_LOADBOOL: u8 = 3;
#[allow(dead_code)]
const OP_GETTABUP: u8 = 6;
#[allow(dead_code)]
const OP_ADD: u8 = 13;
const OP_RETURN: u8 = 38;

/// Build a PUC 5.3 main proto whose body is `LOADK R0 K0; RETURN R0 2`
/// where K0 is the integer 42. Designed to exercise:
/// - header check (LUAC_INT, LUAC_NUM, size bytes)
/// - const-pool LUA_TNUMINT decode
/// - LOADK iABx re-encode
/// - RETURN iABC re-encode
fn build_return_42_chunk() -> Vec<u8> {
    let mut body = Vec::new();
    // main has 1 upvalue (the _ENV cell that `Vm::load` will fill with
    // the globals table). It carries no name in the upvalue table — the
    // debug section names it "_ENV".
    body.push(1u8); // nupvalues for main closure (read before LoadFunction)

    // ---- main proto ----
    body.extend_from_slice(&puc53_str(b"@test")); // source
    put_i32(&mut body, 0); // linedefined
    put_i32(&mut body, 0); // lastlinedefined
    body.push(0); // numparams
    body.push(1); // is_vararg (1 = VARARG_ISVARARG in 5.3)
    body.push(2); // maxstacksize

    // ---- code (2 insts) ----
    put_i32(&mut body, 2);
    put_u32(&mut body, enc_bx(OP_LOADK, 0, 0)); // LOADK R0 K0
    put_u32(&mut body, enc(OP_RETURN, 0, 2, 0)); // RETURN R0 B=2

    // ---- constants (1 int) ----
    put_i32(&mut body, 1);
    body.push(0x13); // LUA_TNUMINT = 3 | (1 << 4) = 19
    put_i64(&mut body, 42);

    // ---- upvalues (1: _ENV at parent stack idx 0) ----
    put_i32(&mut body, 1);
    body.push(1); // instack = true (main's _ENV captured from globals)
    body.push(0); // idx

    // ---- nested protos (0) ----
    put_i32(&mut body, 0);

    // ---- debug: lineinfo (2 entries) ----
    put_i32(&mut body, 2);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    // locvars (0)
    put_i32(&mut body, 0);
    // upvalue names (1: "_ENV")
    put_i32(&mut body, 1);
    body.extend_from_slice(&puc53_str(b"_ENV"));

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&HEADER_53);
    chunk.extend(body);
    chunk
}

/// Same shape but constant is a float 3.5.
fn build_return_float_chunk() -> Vec<u8> {
    let mut body = Vec::new();
    body.push(1u8);
    body.extend_from_slice(&puc53_str(b"@test"));
    put_i32(&mut body, 0);
    put_i32(&mut body, 0);
    body.push(0);
    body.push(1);
    body.push(2);

    put_i32(&mut body, 2);
    put_u32(&mut body, enc_bx(OP_LOADK, 0, 0));
    put_u32(&mut body, enc(OP_RETURN, 0, 2, 0));

    put_i32(&mut body, 1);
    body.push(3); // LUA_TNUMFLT
    put_f64(&mut body, 3.5);

    put_i32(&mut body, 1);
    body.push(1);
    body.push(0);
    put_i32(&mut body, 0);
    put_i32(&mut body, 2);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    put_i32(&mut body, 0);
    put_i32(&mut body, 1);
    body.extend_from_slice(&puc53_str(b"_ENV"));

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&HEADER_53);
    chunk.extend(body);
    chunk
}

// ---- tests ----

#[test]
fn rejects_when_puc_loading_disabled() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    // default puc_bytecode_loading = false
    vm.set_bytecode_loading(true);
    let chunk = build_return_42_chunk();
    let err = vm.load(&chunk, b"=test").unwrap_err();
    let s = format!("{}", String::from_utf8_lossy(&err.msg));
    assert!(
        s.contains("PUC bytecode") || s.contains("disabled"),
        "expected PUC-disabled message, got: {s}"
    );
}

#[test]
fn rejects_bad_luac_int() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let mut chunk = build_return_42_chunk();
    // LUAC_INT lives at header offset 18..26
    chunk[18] = 0xFF;
    let err = vm.load(&chunk, b"=test").unwrap_err();
    let s = String::from_utf8_lossy(&err.msg).into_owned();
    assert_eq!(s, "test: bad binary format (integer format mismatch)");
}

#[test]
fn rejects_bad_sizeof_size_t() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let mut chunk = build_return_42_chunk();
    // sizeof(size_t) at offset 13
    chunk[13] = 4;
    let err = vm.load(&chunk, b"=test").unwrap_err();
    let s = String::from_utf8_lossy(&err.msg).into_owned();
    assert_eq!(s, "test: bad binary format (size_t size mismatch)");
}

#[test]
fn rejects_bad_sizeof_int() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let mut chunk = build_return_42_chunk();
    // sizeof(int) at offset 12
    chunk[12] = 8;
    let err = vm.load(&chunk, b"=test").unwrap_err();
    let s = String::from_utf8_lossy(&err.msg).into_owned();
    assert_eq!(s, "test: bad binary format (int size mismatch)");
}

#[test]
fn rejects_bad_luac_num() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let mut chunk = build_return_42_chunk();
    // LUAC_NUM lives at header offset 25..33; flip the high byte
    chunk[32] = 0xFF;
    let err = vm.load(&chunk, b"=test").unwrap_err();
    let s = String::from_utf8_lossy(&err.msg).into_owned();
    assert_eq!(s, "test: bad binary format (float format mismatch)");
}

#[test]
fn loads_return_int_chunk() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_42_chunk();
    vm.load(&chunk, b"=test").expect("undump succeeds");
}

#[test]
fn loads_return_float_chunk() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_float_chunk();
    vm.load(&chunk, b"=test").expect("undump succeeds");
}

#[test]
fn rejects_wrong_version_byte() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let mut chunk = build_return_42_chunk();
    chunk[4] = 0x52; // pretend to be 5.2 — should route elsewhere
    let err = vm.load(&chunk, b"=test").unwrap_err();
    // the 5.2 reader meets 5.3's LUAC_DATA where 5.2 keeps its endianness
    // flag (header byte 6)
    let s = String::from_utf8_lossy(&err.msg).into_owned();
    assert_eq!(s, "test: bad binary format (integer format mismatch)");
}

/// End-to-end: invoke the loaded chunk and check the returned value is
/// the integer 42. This wires the translator output through the
/// interpreter dispatch (Op::LoadK + Op::Return) so any mistranslation
/// at the bit-layout level shows up as a wrong return / panic / type
/// mismatch.
#[test]
fn end_to_end_returns_42() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_42_chunk();
    let closure = vm.load(&chunk, b"=test").expect("undump");
    let result = vm
        .call_value(Value::Closure(closure), &[])
        .expect("call succeeds");
    assert_eq!(result.len(), 1, "expected one return value");
    match result[0] {
        Value::Int(42) => {}
        other => panic!("expected Int(42), got {other:?}"),
    }
}

#[test]
fn end_to_end_returns_float() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_float_chunk();
    let closure = vm.load(&chunk, b"=test").expect("undump");
    let result = vm
        .call_value(Value::Closure(closure), &[])
        .expect("call succeeds");
    assert_eq!(result.len(), 1);
    match result[0] {
        Value::Float(f) if (f - 3.5).abs() < 1e-9 => {}
        other => panic!("expected Float(3.5), got {other:?}"),
    }
}

/// Build a PUC 5.3 chunk whose body exercises the Phase 4 PU Wave 3
/// **LOADBOOL true+skip** lowering. The body:
///
///   pc 0: LOADBOOL R0 1 1   (R0 = true, skip next)
///   pc 1: LOADBOOL R0 0 0   (R0 = false, skipped)
///   pc 2: RETURN R0 2       (return R0)
///
/// After translation the luna stream is:
///
///   luna 0: LoadTrue R0
///   luna 1: Jmp +1          (skip the LoadFalse at luna 2)
///   luna 2: LoadFalse R0    (the original "skipped" inst)
///   luna 3: Return R0 2
///
/// If the Jmp target remap is wrong the LoadFalse fires and the
/// function returns `false`; the end-to-end assertion catches that.
fn build_return_true_via_loadbool_skip_chunk() -> Vec<u8> {
    let mut body = Vec::new();
    body.push(1u8); // nupvalues

    body.extend_from_slice(&puc53_str(b"@test"));
    put_i32(&mut body, 0); // linedefined
    put_i32(&mut body, 0); // lastlinedefined
    body.push(0); // numparams
    body.push(1); // is_vararg
    body.push(1); // maxstacksize — R0 only

    // ---- code (3 insts) ----
    put_i32(&mut body, 3);
    put_u32(&mut body, enc(OP_LOADBOOL, 0, 1, 1)); // LOADBOOL R0 B=1 C=1
    put_u32(&mut body, enc(OP_LOADBOOL, 0, 0, 0)); // LOADBOOL R0 B=0 C=0 (skipped)
    put_u32(&mut body, enc(OP_RETURN, 0, 2, 0)); // RETURN R0 B=2

    // ---- constants (0) ----
    put_i32(&mut body, 0);

    // ---- upvalues (1: _ENV) ----
    put_i32(&mut body, 1);
    body.push(1);
    body.push(0);

    // ---- nested protos (0) ----
    put_i32(&mut body, 0);

    // ---- debug: lineinfo (3 entries) ----
    put_i32(&mut body, 3);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    put_i32(&mut body, 1);
    // locvars (0)
    put_i32(&mut body, 0);
    // upvalue names (1: "_ENV")
    put_i32(&mut body, 1);
    body.extend_from_slice(&puc53_str(b"_ENV"));

    let mut chunk = Vec::new();
    chunk.extend_from_slice(&HEADER_53);
    chunk.extend(body);
    chunk
}

/// End-to-end smoke for the LOADBOOL true+skip lowering. If the new
/// `LoadTrue; Jmp <skip>` pair lands wrong (Jmp delta off, skipped inst
/// reached) the function returns `false` instead of `true` and the
/// assertion catches it. Pinned alongside `end_to_end_returns_42` so
/// any future regression on the Phase 4 PU Wave 3 punt-lowering surface
/// trips a clear test failure.
#[test]
fn end_to_end_loadbool_true_skip_returns_true() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_true_via_loadbool_skip_chunk();
    let closure = vm.load(&chunk, b"=test").expect("undump");
    let result = vm
        .call_value(Value::Closure(closure), &[])
        .expect("call succeeds");
    assert_eq!(result.len(), 1, "expected one return value");
    match result[0] {
        Value::Bool(true) => {}
        other => panic!("expected Bool(true) (skipped LoadFalse), got {other:?}"),
    }
}
