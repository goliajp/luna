//! The LLVM backend computes on raw integer payloads, so every value it
//! reads has to be an integer. Nil is stored as the payload 0: a trace
//! or chunk comparing an integer 0 with nil found them equal, and a chunk
//! returning nil returned the integer 0. An upvalue of another type was
//! read as its raw bits, and a call through an upvalue that no longer
//! held the running function still went to the running function.

use luna_core::jit::trace_types::{CompileOptions, RecordedOp, TraceRecord};
use luna_core::jit::{CompileResult, IntChunkCompiler, TraceCompiler};
use luna_core::runtime::Value;
use luna_core::runtime::value::raw;
use luna_core::vm::isa::{Inst, Op};
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

fn eq_trace(tags: Vec<u8>) -> Option<luna_core::jit::trace_types::CompiledTrace> {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let proto = vm
        .load(b"local a, b = 1, 2; return a + b", b"=eq_trace")
        .expect("parse")
        .proto;
    let mut record = TraceRecord::start(proto, 0, tags, false);
    for (pc, inst) in [Inst::iabc(Op::Eq, 0, 1, 0, true), Inst::isj(Op::Jmp, -2)]
        .into_iter()
        .enumerate()
    {
        record.push(RecordedOp {
            proto,
            pc: pc as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
    }
    record.closed = true;
    LlvmBackend.try_compile_trace(
        &mut LlvmJitStorage::default(),
        &record,
        CompileOptions::default(),
    )
}

#[test]
fn trace_compares_only_integers() {
    assert!(eq_trace(vec![raw::INT, raw::INT, raw::INT]).is_some());
    assert!(
        eq_trace(vec![raw::INT, raw::NIL, raw::INT]).is_none(),
        "a trace comparing with a nil that entered the trace compiled"
    );
    assert!(eq_trace(vec![raw::FLOAT, raw::INT, raw::INT]).is_none());
}

/// The storage owns the compiled code: keep it while calling `entry`.
fn chunk(src: &[u8]) -> (luna_jit::vm::Vm, LlvmJitStorage, CompileResult) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let proto = vm.load(src, b"=chunk").expect("parse").proto;
    let mut storage = LlvmJitStorage::default();
    let r = LlvmBackend.try_compile(&mut storage, proto, false, false);
    (vm, storage, r)
}

#[test]
fn chunk_integer_zero_is_not_nil() {
    let (_vm, _storage, r) = chunk(b"local x = 0; if x == nil then return 1 else return 2 end");
    if let CompileResult::Compiled { entry, .. } = r {
        let f: unsafe extern "C" fn() -> i64 = unsafe { std::mem::transmute(entry) };
        assert_eq!(unsafe { f() }, 2, "0 == nil took the then-branch");
    }
}

#[test]
fn chunk_returning_nil_is_not_compiled() {
    let (_vm, _storage, r) = chunk(b"local x = nil; return x");
    assert!(
        matches!(r, CompileResult::Skipped),
        "a chunk returning nil compiled (it returns the integer 0)"
    );
}

/// Runs the outer chunk, returns the closure it returns, and compiles
/// that closure's proto into `storage`.
fn inner(
    vm: &mut luna_jit::vm::Vm,
    storage: &mut LlvmJitStorage,
    src: &[u8],
) -> (
    luna_core::runtime::Gc<luna_core::runtime::LuaClosure>,
    *const u8,
) {
    let outer = vm.load(src, b"=outer").expect("parse");
    let r = vm.call_value(Value::Closure(outer), &[]).expect("runs");
    let Some(Value::Closure(cl)) = r.first() else {
        panic!("outer chunk returned {r:?}")
    };
    let CompileResult::Compiled { entry, .. } =
        LlvmBackend.try_compile(storage, cl.proto, false, false)
    else {
        panic!("inner function did not compile")
    };
    (*cl, entry)
}

#[test]
fn chunk_upvalue_of_another_type_deopts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let mut storage = LlvmJitStorage::default();
    let (cl, entry) = inner(
        &mut vm,
        &mut storage,
        b"local k = 1.5; local function f() return k end; return f",
    );
    let _guard = LlvmBackend.enter(&mut vm as *mut _, Some(cl));
    let f: unsafe extern "C" fn() -> i64 = unsafe { std::mem::transmute(entry) };
    unsafe { f() };
    drop(_guard);
    assert!(
        vm.jit.pending_err.is_some(),
        "a float upvalue was read as an integer"
    );
}

#[test]
fn chunk_call_through_a_reassigned_upvalue_deopts() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let mut storage = LlvmJitStorage::default();
    let (cl, entry) = inner(
        &mut vm,
        &mut storage,
        b"local a, b
          b = function(n) return 100 end
          a = function(n) local one = 1 if n < one then return n end local r = b(n - one) return r end
          return a",
    );
    let _guard = LlvmBackend.enter(&mut vm as *mut _, Some(cl));
    let f: unsafe extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(entry) };
    let r = unsafe { f(3) };
    drop(_guard);
    assert!(
        vm.jit.pending_err.is_some(),
        "the call through `b` ran `a` itself (returned {r})"
    );
}
