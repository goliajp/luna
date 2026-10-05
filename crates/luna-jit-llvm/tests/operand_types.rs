//! The LLVM method JIT computes on raw integer payloads, so every value it
//! reads has to be an integer. Nil is stored as the payload 0: a chunk
//! comparing an integer 0 with nil found them equal, and a chunk
//! returning nil returned the integer 0. An upvalue of another type was
//! read as its raw bits, and a call through an upvalue that no longer
//! held the running function still went to the running function.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::runtime::Value;
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

mod support;

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
        // SAFETY: `entry` is from `chunk`'s `try_compile`, for a chunk with
        // no parameters, and `_storage` is still alive
        let got = unsafe { support::call_chunk(entry, &[]) };
        assert_eq!(got, 2, "0 == nil took the then-branch");
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
    // SAFETY: `entry` is from `inner`'s `try_compile`, for a function with
    // no parameters; `storage` is alive and the guard above is held for
    // the upvalue helper
    unsafe { support::call_chunk(entry, &[]) };
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
    // SAFETY: `entry` is from `inner`'s `try_compile`, for a function of
    // one parameter; `storage` is alive and the guard above is held for
    // the upvalue and call helpers
    let r = unsafe { support::call_chunk(entry, &[3]) };
    drop(_guard);
    assert!(
        vm.jit.pending_err.is_some(),
        "the call through `b` ran `a` itself (returned {r})"
    );
}

/// The compiler folds a constant operand into the instruction (`AddI`,
/// `AddK`, `LtI`, `EqK`, …) instead of loading it into a register. The
/// chunk JIT reads those operands from the instruction and the constant
/// table; `src` must contain `op`, and the JIT entry must return what the
/// interpreter returns.
fn jit_matches_interpreter(src: &str, op: Op) -> i64 {
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
fn refused(src: &str, op: Op) {
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

#[test]
fn chunk_immediate_arithmetic() {
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x + 1", Op::AddI),
        6
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x + -127", Op::AddI),
        -122
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x - 128", Op::SubI),
        -123
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x - -3", Op::SubI),
        8
    );
    // the constant on the left sets k; the result is the same
    assert_eq!(
        jit_matches_interpreter("local x = 5; return 7 + x", Op::AddI),
        12
    );
}

#[test]
fn chunk_constant_arithmetic() {
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x + 100000", Op::AddK),
        100005
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x - 100000", Op::SubK),
        -99995
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return x * 3", Op::MulK),
        15
    );
    assert_eq!(
        jit_matches_interpreter("local x = 5; return -3 * x", Op::MulK),
        -15
    );
    assert_eq!(
        jit_matches_interpreter("local x = 17; return x % 5", Op::ModK),
        2
    );
    assert_eq!(
        jit_matches_interpreter("local x = -7; return x % 3", Op::ModK),
        2
    );
    assert_eq!(
        jit_matches_interpreter("local x = 7; return x % -3", Op::ModK),
        -2
    );
    assert_eq!(
        jit_matches_interpreter("local x = -7; return x % -3", Op::ModK),
        -1
    );
}

#[test]
fn chunk_constant_operand_refused_when_not_an_integer() {
    refused("local x = 5; return x % 0", Op::ModK);
    refused("local x = 5; return x + 1.5", Op::AddK);
    refused("local x = 5; return x * 2.5", Op::MulK);
    refused("local x = 5; return x - 100000.5", Op::SubK);
}

#[test]
fn chunk_constant_forms_without_a_register_form_are_refused() {
    refused("local x = 5; return x // 2", Op::IDivK);
    refused("local x = 5; return x / 2", Op::DivK);
    refused("local x = 5; return x ^ 2", Op::PowK);
    refused("local x = 5; return x & 3", Op::BAndK);
    refused("local x = 5; return x | 3", Op::BOrK);
    refused("local x = 5; return x ~ 3", Op::BXorK);
    refused("local x = 5; return x >> 1", Op::ShrI);
    refused("local x = 5; return x << 1", Op::ShlI);
}

#[test]
fn chunk_immediate_comparisons() {
    let cases: &[(&str, Op, i64)] = &[
        (
            "local x = 5; if x < 10 then return 1 else return 0 end",
            Op::LtI,
            1,
        ),
        (
            "local x = 20; if x < 10 then return 1 else return 0 end",
            Op::LtI,
            0,
        ),
        (
            "local x = 5; if x <= 5 then return 1 else return 0 end",
            Op::LeI,
            1,
        ),
        (
            "local x = 6; if x <= 5 then return 1 else return 0 end",
            Op::LeI,
            0,
        ),
        (
            "local x = 5; if x > 4 then return 1 else return 0 end",
            Op::GtI,
            1,
        ),
        (
            "local x = 4; if x > 4 then return 1 else return 0 end",
            Op::GtI,
            0,
        ),
        (
            "local x = 5; if x >= 6 then return 1 else return 0 end",
            Op::GeI,
            0,
        ),
        (
            "local x = 6; if x >= 6 then return 1 else return 0 end",
            Op::GeI,
            1,
        ),
        (
            "local x = 5; if x == 5 then return 1 else return 0 end",
            Op::EqI,
            1,
        ),
        (
            "local x = 5; if x == 6 then return 1 else return 0 end",
            Op::EqI,
            0,
        ),
        (
            "local x = -5; if x > -6 then return 1 else return 0 end",
            Op::GtI,
            1,
        ),
        (
            "local x = -5; if x < -5 then return 1 else return 0 end",
            Op::LtI,
            0,
        ),
        // the constant on the left flips the comparison
        (
            "local x = 5; if 3 < x then return 1 else return 0 end",
            Op::GtI,
            1,
        ),
        (
            "local x = 5; if 5 <= x then return 1 else return 0 end",
            Op::GeI,
            1,
        ),
        (
            "local x = 5; if 5 > x then return 1 else return 0 end",
            Op::LtI,
            0,
        ),
        (
            "local x = 5; if 4 >= x then return 1 else return 0 end",
            Op::LeI,
            0,
        ),
        (
            "local x = 5; while x < 8 do x = x + 1 end return x",
            Op::LtI,
            8,
        ),
    ];
    for &(src, op, want) in cases {
        assert_eq!(jit_matches_interpreter(src, op), want, "{src}");
    }
}

#[test]
fn chunk_constant_comparisons() {
    let src = "local x = 200; if x == 200 then return 1 else return 0 end";
    assert_eq!(jit_matches_interpreter(src, Op::EqK), 1);
    let src = "local x = 5; if x == 100000 then return 1 else return 0 end";
    assert_eq!(jit_matches_interpreter(src, Op::EqK), 0);
    refused(
        "local x = 5; if x == 'a' then return 1 else return 0 end",
        Op::EqK,
    );
    refused(
        "local x = 5; if x == 1.5 then return 1 else return 0 end",
        Op::EqK,
    );
    // an integer-valued float immediate is a float compare
    refused(
        "local x = 5; if x == 5.0 then return 1 else return 0 end",
        Op::EqI,
    );
    refused(
        "local x = 5; if x < 6.0 then return 1 else return 0 end",
        Op::LtI,
    );
}

#[test]
fn chunk_cache_key_covers_the_constants() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let mut storage = LlvmJitStorage::default();
    let mut compile = |src: &[u8]| {
        let proto = vm.load(src, b"=chunk").expect("parse").proto;
        assert!(proto.code.iter().any(|i| i.op() == Op::AddK));
        let CompileResult::Compiled { entry, .. } =
            LlvmBackend.try_compile(&mut storage, proto, false, false)
        else {
            panic!("did not compile")
        };
        // SAFETY: `entry` is from the `try_compile` above, for a chunk with
        // no parameters, and `storage` lives until the end of the test
        unsafe { support::call_chunk(entry, &[]) }
    };
    assert_eq!(compile(b"local x = 5; return x + 100000"), 100005);
    assert_eq!(
        compile(b"local x = 5; return x + 200000"),
        200005,
        "same bytecode, different constant: served from the cache"
    );
}
