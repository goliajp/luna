//! The nested-branch shape of a fib body.

use luna_core::jit::{CompileResult, IntChunkCompiler};
use luna_core::vm::isa::Op;
use luna_jit::LuaVersion;
use luna_jit_llvm::{LlvmBackend, LlvmJitStorage};

/// Substantial int chunk that exercises every non-call op the
/// compute path supports: `LoadI` / `Move` / `Add` / `Sub` / `Mul` /
/// `Mod` / `Lt` / `Le` / `Eq` / `Jmp` / `Return1`. A branchy stand-in
/// for a "fib(N) shape" — iterative `fib` needs `Op::ForPrep` /
/// `Op::ForLoop`, which the compute path does not lower.
///
/// Chunk:
/// ```lua
/// local n = 7
/// if n < 5 then
///   return n * n
/// else
///   local d = n - 3        -- 4
///   if d == 4 then
///     return d + 100       -- 104
///   else
///     return d % 3
///   end
/// end
/// ```
/// For n=7: 7<5 false → else; d = 4; d==4 true → return 104.
#[test]
fn fib_shape_nested_branchy_chunk() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 7\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested",
        )
        .expect("compile");
    let proto = closure.proto;

    // Confirm the chunk really exercises the breadth of ops the test
    // claims, constant operands in their immediate / constant forms.
    // If the parser ever folds any of these out, surface that loudly.
    for op in [
        Op::LoadI,
        Op::LtI,
        Op::EqI,
        Op::Mul,
        Op::SubI,
        Op::ModK,
        Op::AddI,
        Op::Jmp,
        Op::Return1,
    ] {
        assert!(
            proto.code.iter().any(|i| i.op() == op),
            "fib-shape chunk must exercise {op:?}; code = {:?}",
            proto.code,
        );
    }

    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled {
        entry, returns_one, ..
    } = backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("fib-shape chunk must compile via compute path");
    };
    assert!(returns_one);
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 104, "n=7 → else → d=4 → d==4 → return 104",);
}

/// Same chunk, then-branch path (n=3):
/// `3 < 5` true → return `3 * 3` = 9.
#[test]
fn fib_shape_nested_branchy_chunk_then_path() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 3\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested_then",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("then-branch chunk must compile");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 9);
}

/// Deepest else-branch (n=11 → else → d=8 →
/// d==4 false → return `d % 3 == 2`).
#[test]
fn fib_shape_nested_branchy_chunk_deep_else() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let closure = vm
        .load(
            b"local n = 11\n\
              if n < 5 then\n\
                return n * n\n\
              else\n\
                local d = n - 3\n\
                if d == 4 then\n\
                  return d + 100\n\
                else\n\
                  return d % 3\n\
                end\n\
              end",
            b"=fib_shape_nested_deep_else",
        )
        .expect("compile");
    let proto = closure.proto;
    let backend = LlvmBackend;
    let mut storage = LlvmJitStorage::default();
    let CompileResult::Compiled { entry, .. } =
        backend.try_compile(&mut storage, proto, false, false)
    else {
        panic!("deep-else chunk must compile");
    };
    // SAFETY: `entry` is from the `try_compile` above, for a chunk with no
    // parameters, and `storage` is still alive
    let returned = unsafe { crate::support::call_chunk(entry, &[]) };
    assert_eq!(returned, 2);
}
