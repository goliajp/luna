//! Phase LB Wave 2 — PUC Lua 5.4 `.luac` undumper end-to-end tests.
//!
//! Validates `crates/luna-core/src/vm/dump/puc/puc_54.rs` against real
//! `luac5.4`-compiled bytecode. The bulk of the per-piece coverage (varint,
//! header validation, RLE lineinfo, opcode translation, MMBIN drop, K/I-imm
//! lowering, PC remap) lives next to the implementation in
//! `vm::dump::puc::puc_54::tests`; this file is the end-to-end "compile via
//! PUC, load + run via luna" suite.
//!
//! The `compile_via_puc` helper shells out to `luac5.4` and skips
//! gracefully if it's not installed — keeping CI green on lean runners.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::process::Command;

/// Try to invoke `luac5.4` with the given Lua source and return the
/// compiled bytecode. Returns `None` if `luac5.4` is not available.
fn compile_via_puc(src: &str) -> Option<Vec<u8>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    src.hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let dir = std::env::temp_dir().join(format!(
        "luna-puc54-undump-{}-{:x}",
        std::process::id(),
        h.finish()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let in_path = dir.join("in.lua");
    let out_path = dir.join("out.luac");
    std::fs::write(&in_path, src).ok()?;
    let status = Command::new("luac5.4")
        .arg("-o")
        .arg(&out_path)
        .arg(&in_path)
        .status();
    let Ok(status) = status else {
        return None; // luac5.4 not on PATH
    };
    if !status.success() {
        eprintln!("luac5.4 returned non-zero status: {status}");
        return None;
    }
    std::fs::read(&out_path).ok()
}

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
        msg.contains("bad PUC 5.4 chunk header") || msg.contains("not a PUC 5.4"),
        "unexpected error: {msg}"
    );
}

#[test]
fn undumps_const_return() {
    let Some(luac) = compile_via_puc("return 42") else {
        eprintln!("luac5.4 not available; skipping puc_54_undump::undumps_const_return");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    let cl = vm
        .load(&luac, b"=t")
        .expect("PUC 5.4 chunk should load with gate on");
    let res = vm
        .call_value(Value::Closure(cl), &[])
        .expect("PUC 5.4 chunk should run");
    assert_eq!(res.len(), 1, "expected single return, got {res:?}");
    assert!(
        matches!(res[0], Value::Int(42) | Value::Float(_)),
        "expected 42, got {:?}",
        res[0]
    );
}

#[test]
fn undumps_arithmetic_chain() {
    // Exercises OP_ADDK + (PUC 5.4 emits) MMBIN ops, which the translator
    // must drop.
    let Some(luac) = compile_via_puc("return 1 + 2 + 3") else {
        eprintln!("luac5.4 not available; skipping puc_54_undump::undumps_arithmetic_chain");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    let cl = vm.load(&luac, b"=t").expect("load");
    let res = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let n = match res[0] {
        Value::Int(n) => n as f64,
        Value::Float(f) => f,
        _ => panic!("expected number, got {:?}", res[0]),
    };
    assert_eq!(n, 6.0);
}

#[test]
fn undumps_function_with_call() {
    // Exercises OP_CLOSURE, OP_CALL, OP_RETURN, OP_VARARGPREP (drop),
    // sub-protos.
    let src = r#"
        local function add(a, b)
            return a + b
        end
        return add(10, 32)
    "#;
    let Some(luac) = compile_via_puc(src) else {
        eprintln!("luac5.4 not available; skipping puc_54_undump::undumps_function_with_call");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    let cl = vm.load(&luac, b"=t").expect("load");
    let res = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let n = match res[0] {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        _ => panic!("expected number, got {:?}", res[0]),
    };
    assert_eq!(n, 42);
}

#[test]
fn proto_has_expected_constants() {
    let Some(luac) = compile_via_puc(r#"return "hello", 3.14, 100"#) else {
        eprintln!("luac5.4 not available; skipping puc_54_undump::proto_has_expected_constants");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    let cl = vm.load(&luac, b"=t").expect("load");
    let proto = cl.proto;
    let consts = &proto.consts;
    let has_str = consts
        .iter()
        .any(|v| matches!(v, Value::Str(s) if s.as_bytes() == b"hello"));
    // Fixture's 5.4 .luac chunk encodes the literal 3.14 — not
    // std::f64::consts::PI. allow approx_constant for this assertion only.
    #[allow(clippy::approx_constant)]
    let pi_fixture: f64 = 3.14;
    let has_pi = consts
        .iter()
        .any(|v| matches!(v, Value::Float(f) if (*f - pi_fixture).abs() < 1e-9));
    let has_int100 = consts.iter().any(|v| match v {
        Value::Int(100) => true,
        Value::Float(f) => *f == 100.0,
        _ => false,
    });
    assert!(has_str, "missing 'hello' in consts {consts:?}");
    assert!(has_pi, "missing 3.14 in consts {consts:?}");
    assert!(has_int100, "missing 100 in consts {consts:?}");
    assert!(!proto.code.is_empty(), "code stream is empty");
}

#[test]
fn undumps_numeric_for_loop() {
    // Exercises OP_FORPREP / OP_FORLOOP — both 1:1 from PUC 5.4 to luna.
    let src = r#"
        local s = 0
        for i = 1, 10 do s = s + i end
        return s
    "#;
    let Some(luac) = compile_via_puc(src) else {
        eprintln!("luac5.4 not available; skipping puc_54_undump::undumps_numeric_for_loop");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_puc_bytecode_loading(true);
    let cl = vm.load(&luac, b"=t").expect("load");
    let res = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let n = match res[0] {
        Value::Int(n) => n,
        Value::Float(f) => f as i64,
        _ => panic!("expected number, got {:?}", res[0]),
    };
    assert_eq!(n, 55);
}
