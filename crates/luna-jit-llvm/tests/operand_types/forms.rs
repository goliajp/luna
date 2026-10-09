//! Running a chunk whose code holds a constant operand form under the
//! LLVM method JIT, against the interpreter.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::runtime::Value;
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

use super::support;

/// The compiler folds a constant operand into the instruction (`AddI`,
/// `AddK`, `LtI`, `EqK`, …) instead of loading it into a register. The
/// chunk JIT reads those operands from the instruction and the constant
/// table; `src` must contain `op`, and the JIT entry must return what the
/// interpreter returns.
pub(crate) fn jit_matches_interpreter(src: &str, op: Op) -> i64 {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let cl = vm.load(src.as_bytes(), b"=chunk").expect("parse");
    let proto = cl.proto;
    assert!(
        proto.code.iter().any(|i| i.op() == op),
        "{src}: expected {op:?}, got {:?}",
        proto.code
    );
    let expected = match vm
        .call_value(Value::Closure(cl), &[])
        .expect("runs")
        .first()
    {
        Some(Value::Int(i)) => *i,
        other => panic!("{src}: interpreter returned {other:?}"),
    };
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        LlvmBackend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("{src}: did not compile ({:?})", proto.code)
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let got = unsafe { support::call_chunk(entry, &[]) };
    assert_eq!(got, expected, "{src}: jit {got}, interpreter {expected}");
    got
}

/// `src` contains `op` and the chunk JIT refuses it.
pub(crate) fn refused(src: &str, op: Op) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let proto = vm.load(src.as_bytes(), b"=chunk").expect("parse").proto;
    assert!(
        proto.code.iter().any(|i| i.op() == op),
        "{src}: expected {op:?}, got {:?}",
        proto.code
    );
    let r = LlvmBackend.try_compile(&mut LlvmJitStorage::default(), proto, false, false);
    assert!(matches!(r, CompileResult::Skipped), "{src}: compiled");
}
