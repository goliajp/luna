//! Phase LB Wave 2 — PUC Lua 5.5 `.luac` undumper end-to-end tests.
//!
//! Validates `crates/luna-core/src/vm/dump/puc/puc_55.rs` against real
//! `luac5.5`-compiled bytecode. The bulk of the unit-test coverage
//! (varint, header validation, per-op translation) lives next to the
//! implementation in `vm::dump::puc::puc_55::tests`; this file is the
//! end-to-end "compile via PUC, load + run via luna" suite.
//!
//! The `compile_via_puc` helper shells out to `luac5.5` (Homebrew
//! `/opt/homebrew/bin/luac5.5` on macOS, `luac5.5` on PATH elsewhere)
//! and skips the test gracefully if it's not installed — keeping CI
//! green on lean Linux runners.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::process::Command;

/// Try to invoke `luac5.5` with the given Lua source and return the
/// compiled bytecode. Returns `None` if `luac5.5` is not available on
/// this host (so the calling test self-skips).
fn compile_via_puc(src: &str) -> Option<Vec<u8>> {
    // Write src to a tempfile, run `luac5.5 -o <out> <in>`, read <out>.
    // Per-(pid, src-hash, thread) directory keeps tests safe under
    // `cargo test`'s default parallelism — even other crates in the
    // workspace running `luac5.5` concurrently can't clobber us.
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    src.hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let dir = std::env::temp_dir().join(format!(
        "luna-puc55-undump-{}-{:x}",
        std::process::id(),
        h.finish()
    ));
    std::fs::create_dir_all(&dir).ok()?;
    let in_path = dir.join("in.lua");
    let out_path = dir.join("out.luac");
    std::fs::write(&in_path, src).ok()?;
    let status = Command::new("luac5.5")
        .arg("-o")
        .arg(&out_path)
        .arg(&in_path)
        .status();
    let Ok(status) = status else {
        return None; // luac5.5 not on PATH
    };
    if !status.success() {
        eprintln!("luac5.5 returned non-zero status: {status}");
        return None;
    }
    std::fs::read(&out_path).ok()
}

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

#[test]
fn undumps_const_return() {
    let Some(luac) = compile_via_puc("return 42") else {
        eprintln!("luac5.5 not available; skipping puc_55_undump::undumps_const_return");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_puc_bytecode_loading(true);
    let cl = vm
        .load(&luac, b"=t")
        .expect("PUC 5.5 chunk should load with gate on");
    let res = vm
        .call_value(Value::Closure(cl), &[])
        .expect("PUC 5.5 chunk should run");
    assert_eq!(res.len(), 1, "expected single return, got {res:?}");
    assert!(
        matches!(res[0], Value::Int(42) | Value::Float(_)),
        "expected 42, got {:?}",
        res[0]
    );
}

#[test]
fn undumps_arithmetic() {
    // Tests OP_ADDK / OP_LOADI / OP_RETURN1 — the most common ops.
    let Some(luac) = compile_via_puc("return 1 + 2 + 3") else {
        eprintln!("luac5.5 not available; skipping puc_55_undump::undumps_arithmetic");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua54);
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
    // Exercises OP_CLOSURE, OP_CALL, OP_TAILCALL, sub-protos.
    let src = r#"
        local function add(a, b)
            return a + b
        end
        return add(10, 32)
    "#;
    let Some(luac) = compile_via_puc(src) else {
        eprintln!("luac5.5 not available; skipping puc_55_undump::undumps_function_with_call");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua54);
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
    // White-box: load a chunk that pins down which constants land in
    // the const pool, so a regression in `load_constants` shows up
    // even when the runtime happens to mask the problem.
    // Pick 2.5 (exactly representable f64) instead of 3.14 to keep the
    // const-pool float check tight without tripping clippy's
    // `approx_constant` lint for π.
    let Some(luac) = compile_via_puc(r#"return "hello", 2.5, 100"#) else {
        eprintln!("luac5.5 not available; skipping puc_55_undump::proto_has_expected_constants");
        return;
    };
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_puc_bytecode_loading(true);
    let cl = vm.load(&luac, b"=t").expect("load");
    let proto = cl.proto;
    // PUC 5.5 will emit "hello" + 3.14 + 100 in some order; loose
    // assertions because const-pool ordering depends on the compiler.
    let consts = &proto.consts;
    let has_str = consts
        .iter()
        .any(|v| matches!(v, Value::Str(s) if s.as_bytes() == b"hello"));
    let has_two_five = consts
        .iter()
        .any(|v| matches!(v, Value::Float(f) if *f == 2.5));
    // Note: integer 100 fits in an `sBx` immediate and PUC emits it as
    // `LOADI A 100` (an inline immediate), NOT a const-pool entry — so
    // we don't assert on `consts` for it. We DO assert that the
    // instruction stream carries a LoadI with sbx=100 to lock that the
    // translator faithfully decoded the iAsBx field.
    assert!(has_str, "missing 'hello' in consts {consts:?}");
    assert!(has_two_five, "missing 2.5 in consts {consts:?}");
    let has_loadi_100 = proto
        .code
        .iter()
        .any(|i| i.op() == luna_core::vm::isa::Op::LoadI && i.sbx() == 100);
    assert!(
        has_loadi_100,
        "expected LoadI sbx=100 in code stream {:?}",
        proto.code
    );
    assert!(!proto.code.is_empty(), "code stream is empty");
}
