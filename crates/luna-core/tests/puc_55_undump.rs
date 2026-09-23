//! PUC 5.5 chunk loading: the gate, on hand-made bytes.
//!
//! What a real `luac 5.5` chunk does in luna is pinned by
//! `diff_puc.rs::diff_puc_bytecode`, which runs the whole diff_puc corpus
//! through stock `luac` (`PUC_LUAC_55`) and compares with PUC.

use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[test]
fn header_rejects_when_puc_loading_disabled() {
    // Even without `luac5.5` we can validate the gate: a fabricated 5.5
    // header byte stream that LOOKS like PUC bytecode but isn't enabled
    // for loading must fail with the explicit gate-disabled message
    // (not silently fall through to luna's own undump path).
    let mut vm = Vm::new(LuaVersion::Lua54); // not 5.5: makes 0x55 chunk foreign
    // gate is OFF by default; do not flip it
    let mut bytes = vec![0x1b, b'L', b'u', b'a', 0x55];
    bytes.extend_from_slice(&[0u8; 64]); // junk; loader rejects long before
    let err = vm.load(&bytes, b"=t").expect_err("must reject");
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("PUC bytecode loading is disabled"),
        "unexpected error: {msg}"
    );
}
