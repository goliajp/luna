//! P09 — embedding-API tests for the script-host sandbox pattern.
//!
//! These are the Rust-API analogue of `capi.rs` (which covers the C
//! ABI shim). The use case is a Redis-style script host:
//! per-request short-lived Vms loaded with a curated stdlib subset,
//! instruction + memory budgets to bound runaways, and host-registered
//! Rust callbacks that bridge into the surrounding service.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::error::LuaError;

fn sandbox_51() -> Vm {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua51);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    vm.open_coroutine();
    vm
}

fn run(vm: &mut Vm, src: &[u8]) -> Result<Vec<Value>, LuaError> {
    let cl = vm.load(src, b"=t").expect("compile");
    vm.call_value(Value::Closure(cl), &[])
}

#[test]
fn sandbox_excludes_os_io_debug_package() {
    let mut vm = sandbox_51();
    // Whitelisted libs DO load.
    assert!(matches!(
        run(
            &mut vm,
            b"return type(math), type(string), type(table), type(coroutine)"
        )
        .unwrap()
        .as_slice(),
        [Value::Str(_), Value::Str(_), Value::Str(_), Value::Str(_)]
    ));
    // os/io/debug/package stay nil — type() returns the string "nil".
    let r = run(
        &mut vm,
        b"return type(os), type(io), type(debug), type(package)",
    )
    .unwrap();
    for v in &r {
        match v {
            Value::Str(s) => assert_eq!(s.as_bytes(), b"nil", "got {:?}", s.as_bytes()),
            other => panic!("expected Str('nil'), got {other:?}"),
        }
    }
}

#[test]
fn sandbox_instruction_budget_halts_infinite_loop() {
    let mut vm = sandbox_51();
    vm.set_instr_budget(Some(10_000));
    let err = run(&mut vm, b"while true do end").unwrap_err();
    let msg = vm.error_text(&err);
    assert!(
        msg.contains("instruction budget exceeded"),
        "unexpected error: {msg}"
    );
}

#[test]
fn sandbox_memory_cap_halts_alloc_bomb() {
    let mut vm = sandbox_51();
    vm.set_memory_cap(Some(64 * 1024));
    // 1M concat-cycles will blow past 64 KiB long before completing.
    let err = run(
        &mut vm,
        b"local t = {}; for i = 1, 1000000 do t[i] = string.rep('x', 100) end",
    )
    .unwrap_err();
    let msg = vm.error_text(&err);
    assert!(msg.contains("memory cap exceeded"), "unexpected: {msg}");
}

#[test]
fn sandbox_pcall_traps_script_error() {
    let mut vm = sandbox_51();
    let r = run(
        &mut vm,
        b"local ok, err = pcall(function() error('boom') end); return ok, err",
    )
    .unwrap();
    assert!(matches!(r.as_slice(), [Value::Bool(false), Value::Str(_)]));
}

#[test]
fn sandbox_host_callback_registered() {
    fn host_get(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
        let key = vm.nat_arg(fs, nargs, 0);
        let answer = match key {
            Value::Str(s) if s.as_bytes() == b"life" => Value::Int(42),
            _ => Value::Nil,
        };
        Ok(vm.nat_return(fs, &[answer]))
    }
    let mut vm = sandbox_51();
    let f = vm.native(host_get);
    vm.set_global("host_get", f).unwrap();
    let r = run(&mut vm, b"return host_get('life'), host_get('other')").unwrap();
    assert!(matches!(r.as_slice(), [Value::Int(42), Value::Nil]));
}

#[test]
fn sandbox_jit_disabled_meters_counted_for() {
    // Without `set_jit_enabled(false)` the JIT compiles counted-for to
    // native Cranelift IR that does not tick instr_budget — the host must
    // disable JIT so a malicious `for i=1,1e18 do end` is bounded.
    let mut vm = sandbox_51();
    vm.set_jit_enabled(false);
    vm.set_instr_budget(Some(50_000));
    let err = run(&mut vm, b"for i = 1, 1000000000 do end").unwrap_err();
    let msg = vm.error_text(&err);
    assert!(
        msg.contains("instruction budget exceeded"),
        "JIT-bypass: expected budget exceeded, got: {msg}"
    );
}

#[test]
fn sandbox_rejects_bytecode_when_gated() {
    // `\27Lua` is the binary-chunk signature. With the gate off the
    // loader hands such input to undump (the attack surface the host wants
    // closed); with the gate on it surfaces a clean syntax error.
    let mut vm = sandbox_51();
    vm.set_bytecode_loading(false);
    let header = b"\x1bLua\x55";
    let err = vm.load(header, b"=bc").unwrap_err();
    let msg = String::from_utf8_lossy(&err.msg);
    assert!(
        msg.contains("binary chunk") && msg.contains("disabled"),
        "expected binary-chunk rejection, got: {msg}"
    );
}

#[test]
fn sandbox_51_excludes_string_pack() {
    // string.pack/unpack/packsize landed in 5.3 — 5.1 hosts must
    // not see them.
    let mut vm = sandbox_51();
    let r = run(
        &mut vm,
        b"return string.pack, string.unpack, string.packsize",
    )
    .unwrap();
    for v in &r {
        assert!(
            matches!(v, Value::Nil),
            "5.1 should not have string.pack family, got {v:?}"
        );
    }
}

#[test]
fn sandbox_error_traceback_exposed() {
    let mut vm = sandbox_51();
    let err = run(&mut vm, b"local function inner() error('boom') end inner()").unwrap_err();
    assert!(vm.error_text(&err).contains("boom"));
    let tb = vm.take_error_traceback().expect("traceback captured");
    assert!(tb.contains("in local 'inner'"), "tb: {tb}");
    assert!(tb.contains("in main chunk"), "tb: {tb}");
}

#[test]
fn sandbox_budget_clears_after_exhaustion() {
    // The dispatcher's `instr_budget` is cleared to `None` after firing
    // once (see `Vm::run`'s `self.instr_budget = None;` on exhaustion),
    // so the next call against the same Vm with NO new budget runs
    // freely. Embedders are expected to call `set_instr_budget` again
    // per request — the host resets it on each invocation.
    let mut vm = sandbox_51();
    vm.set_instr_budget(Some(50));
    let err = run(&mut vm, b"while true do end").unwrap_err();
    assert!(vm.error_text(&err).contains("instruction budget exceeded"));
    // After firing, the budget is cleared — a fresh call runs free.
    let r = run(
        &mut vm,
        b"local s = 0; for i = 1, 100 do s = s + i end; return s",
    )
    .expect("clean run after budget clear");
    // 5.1 has no Int subtype — sum is a Float.
    match r.first() {
        Some(&Value::Float(f)) if (f - 5050.0).abs() < 1e-9 => {}
        Some(&Value::Int(5050)) => {}
        other => panic!("expected 5050, got {other:?}"),
    }
}
