//! `LOADBOOL A 1 1` (true, then skip the next instruction) runs as one
//! instruction.

use super::*;

/// Build a PUC 5.3 chunk whose body exercises the
/// **LOADBOOL true+skip** lowering. The body:
///
///   pc 0: LOADBOOL R0 1 1   (R0 = true, skip next)
///   pc 1: LOADBOOL R0 0 0   (R0 = false, skipped)
///   pc 2: RETURN R0 2       (return R0)
///
/// After translation the luna stream is one instruction per PUC one:
///
///   luna 0: LTrueSkip R0
///   luna 1: LoadFalse R0    (skipped)
///   luna 2: Return R0 2
///
/// If the skip went wrong the LoadFalse fires and the function returns
/// `false`; the end-to-end assertion catches that.
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

/// End-to-end smoke for the LOADBOOL true+skip lowering: the skipped
/// instruction is not reached, so the function returns `true`.
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

/// A count hook sees `LOADBOOL 1 1` as one instruction, as PUC runs it: the
/// function executes two instructions (the `LOADBOOL` and the `RETURN`).
#[test]
fn loadbool_true_skip_counts_as_one_instruction() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_bytecode_loading(true);
    vm.set_puc_bytecode_loading(true);
    let chunk = build_return_true_via_loadbool_skip_chunk();
    let closure = vm.load(&chunk, b"=test").expect("undump");
    vm.set_global("f", Value::Closure(closure)).expect("global");
    let r = vm
        .eval(
            "local n = 0\n\
             debug.sethook(function()\n\
               if debug.getinfo(2, 'f').func == f then n = n + 1 end\n\
             end, '', 1)\n\
             local r = f()\n\
             debug.sethook()\n\
             return n, r",
        )
        .expect("runs");
    assert!(matches!(r[..], [Value::Int(2), Value::Bool(true)]), "{r:?}");
}
