//! PUC 5.4 chunk loading: the gate and header diagnostics, on hand-made
//! bytes.
//!
//! What a real `luac 5.4` chunk does in luna is pinned by
//! `diff_puc.rs::diff_puc_bytecode`, which runs the whole diff_puc corpus
//! through stock `luac` (`PUC_LUAC_54`) and compares with PUC.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[test]
fn header_rejects_when_puc_loading_disabled() {
    // Even without `luac5.4` we can validate the gate.
    let mut vm = Vm::new(LuaVersion::Lua55); // not 5.4: makes 0x54 chunk foreign
    let mut bytes = vec![0x1b, b'L', b'u', b'a', 0x54];
    bytes.extend_from_slice(&[0u8; 64]);
    let err = vm.load(&bytes, b"=t").expect_err("must reject");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("PUC bytecode loading is disabled"),
        "unexpected error: {msg}"
    );
}

#[test]
fn header_rejects_corrupted_chunk_with_gate_on() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    // valid signature + version, then junk where LUAC_DATA should be.
    let mut bytes = vec![0x1b, b'L', b'u', b'a', 0x54, 0x00];
    bytes.extend_from_slice(&[0xFF; 64]);
    let err = vm.load(&bytes, b"=t").expect_err("must reject");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("PUC 5.4 chunk: corrupted LUAC_DATA"),
        "unexpected error: {msg}"
    );
}
