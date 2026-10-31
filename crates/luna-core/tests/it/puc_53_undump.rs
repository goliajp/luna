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

mod chunk;
use chunk::*;

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
    chunk[13] = if std::mem::size_of::<usize>() == 8 {
        4
    } else {
        8
    };
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

mod loadbool;
