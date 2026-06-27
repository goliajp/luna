//! v2.1 Phase 1K.B — LLVM JIT toolchain validation harness.
//!
//! Standalone, `publish = false` validation crate for the LLVM 18 +
//! `inkwell` 0.9 toolchain selected by Phase 1K.A
//! (see `.dev/rfcs/v2.1-phase-1k-a-llvm-jit-selection.md`). The crate
//! JIT-compiles a trivial `add(i64, i64) -> i64` IR and invokes it,
//! proving the toolchain links + runs on the dev host before Phase
//! 1K.C+ touches any production luna code.
//!
//! The crate intentionally has no dependency on `luna-core`,
//! `luna-jit`, or `luna-aot`. The 0-third-party-dep contract on
//! `luna-core` is unaffected.
//!
//! Build:
//!     LLVM_SYS_181_PREFIX=/opt/homebrew/opt/llvm@18 \
//!         cargo build -p llvm-jit-probe
//!
//! Test:
//!     LLVM_SYS_181_PREFIX=/opt/homebrew/opt/llvm@18 \
//!         cargo test  -p llvm-jit-probe

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::execution_engine::JitFunction;

/// C-ABI signature of the JIT-compiled `add` function. Calling this is
/// innately `unsafe` because it dereferences a function pointer produced
/// by LLVM at runtime — Rust cannot statically verify the IR's calling
/// convention matches this declaration.
type AddFunc = unsafe extern "C" fn(i64, i64) -> i64;

/// Phase 1K.B.4 — JIT-compile `fn add(a, b) = a + b` in LLVM IR, invoke
/// it with the supplied arguments, and return the result.
///
/// Returns `Err(String)` if the JIT engine cannot be constructed (e.g.
/// LLVM 18 runtime missing) or the function symbol cannot be resolved.
///
/// The IR shape produced is approximately:
///
/// ```llvm
/// define i64 @add(i64 %a, i64 %b) {
/// entry:
///   %sum = add i64 %a, %b
///   ret i64 %sum
/// }
/// ```
pub fn jit_add(a: i64, b: i64) -> Result<i64, String> {
    let context = Context::create();
    let module = context.create_module("llvm_jit_probe_add");
    let builder = context.create_builder();

    let execution_engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .map_err(|e| format!("create_jit_execution_engine: {e}"))?;

    let i64_type = context.i64_type();
    let fn_type = i64_type.fn_type(&[i64_type.into(), i64_type.into()], false);
    let function = module.add_function("add", fn_type, None);
    let basic_block = context.append_basic_block(function, "entry");
    builder.position_at_end(basic_block);

    let a_arg = function
        .get_nth_param(0)
        .ok_or_else(|| "missing param 0".to_string())?
        .into_int_value();
    let b_arg = function
        .get_nth_param(1)
        .ok_or_else(|| "missing param 1".to_string())?
        .into_int_value();

    let sum = builder
        .build_int_add(a_arg, b_arg, "sum")
        .map_err(|e| format!("build_int_add: {e}"))?;
    builder
        .build_return(Some(&sum))
        .map_err(|e| format!("build_return: {e}"))?;

    // SAFETY: `get_function` reads the JIT engine's symbol table for the
    // function we just emitted, casting the returned pointer to the
    // `AddFunc` C-ABI type. The cast is sound because the IR above
    // declares `add` with exactly `(i64, i64) -> i64` and LLVM's
    // default JIT calling convention on aarch64 / x86_64 matches Rust's
    // `extern "C"` for primitive integer arguments + return.
    let add_fn: JitFunction<AddFunc> = unsafe {
        execution_engine
            .get_function("add")
            .map_err(|e| format!("get_function: {e}"))?
    };

    // SAFETY: invoking the JIT-compiled function dereferences a code
    // pointer owned by `execution_engine`. The engine remains alive for
    // the duration of this scope, so the pointer is valid. The argument
    // types match the IR signature declared above.
    let result = unsafe { add_fn.call(a, b) };

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_two_three_is_five() {
        let result = jit_add(2, 3).expect("JIT compile failed");
        assert_eq!(result, 5);
    }

    #[test]
    fn add_negative_operands() {
        let result = jit_add(-7, 3).expect("JIT compile failed");
        assert_eq!(result, -4);
    }

    #[test]
    fn add_wraps_on_overflow() {
        // LLVM `add` (without nsw/nuw) wraps in two's complement.
        let result = jit_add(i64::MAX, 1).expect("JIT compile failed");
        assert_eq!(result, i64::MIN);
    }
}
