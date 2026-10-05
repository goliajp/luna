//! End-to-end smoke for the LLVM backend's
//! simplest recognised chunk shape (`[Op::LoadNil(_, _), Op::Return0]`).
//!
//! The test:
//! 1. Spins up a `luna_jit` Vm with the default Cranelift backend (we
//!    only need its parser + heap to materialise a `Proto`; the JIT
//!    backend choice is irrelevant — we call `LlvmBackend::try_compile`
//!    directly, bypassing the dispatcher).
//! 2. Loads the Lua source `local x` which compiles to a Proto whose
//!    body is exactly `[LoadNil(R0, 0), Return0]`.
//! 3. Calls `LlvmBackend::try_compile` on that Proto.
//! 4. Asserts the result is `CompileResult::Compiled` with the
//!    expected metadata (zero args, no return value, no float / table
//!    masks).
//! 5. Calls the returned entry pointer as an
//!    `extern "C" fn() -> i64`; asserts the
//!    call returns 0 (the chunk has no observable return value).
//!
//! This proves the inkwell → LLVM 18 toolchain emits + JIT-compiles +
//! resolves a Rust-callable function pointer through luna's trait
//! surface, with the per-`Vm` storage cache keeping the Context /
//! ExecutionEngine (and so the mmap) alive for the duration of the
//! test.

#[path = "support/mod.rs"]
mod support;

#[path = "llvm_smoke/arith.rs"]
mod arith;
#[path = "llvm_smoke/branches.rs"]
mod branches;
#[path = "llvm_smoke/dead_locals.rs"]
mod dead_locals;
#[path = "llvm_smoke/fib_shape.rs"]
mod fib_shape;
#[path = "llvm_smoke/mod_guards.rs"]
mod mod_guards;
#[path = "llvm_smoke/moves_and_jumps.rs"]
mod moves_and_jumps;
#[path = "llvm_smoke/recursion.rs"]
mod recursion;
