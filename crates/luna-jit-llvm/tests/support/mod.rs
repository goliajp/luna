//! Calling a chunk the LLVM backend compiled.

/// Call the entry of a compiled chunk with `args` as its parameters.
///
/// # Safety
/// `entry` is the entry of a `CompileResult::Compiled` that
/// `LlvmBackend::try_compile` returned for a chunk of `args.len()`
/// parameters, the `LlvmJitStorage` it was compiled into is still alive,
/// and a guard from `LlvmBackend::enter` is held if the chunk calls a
/// `luna_jit_*` helper.
pub unsafe fn call_chunk(entry: *const u8, args: &[i64]) -> i64 {
    use std::mem::transmute;
    // SAFETY: by the contract, `entry` points at live code compiled as
    // `i64 (i64 × args.len())` with the C calling convention, which is
    // the function pointer type picked for that arity; the helpers it
    // may call find the Vm through the guard
    unsafe {
        match *args {
            [] => transmute::<*const u8, extern "C" fn() -> i64>(entry)(),
            [a] => transmute::<*const u8, extern "C" fn(i64) -> i64>(entry)(a),
            [a, b] => transmute::<*const u8, extern "C" fn(i64, i64) -> i64>(entry)(a, b),
            [a, b, c] => {
                transmute::<*const u8, extern "C" fn(i64, i64, i64) -> i64>(entry)(a, b, c)
            }
            _ => panic!("no test chunk takes {} parameters", args.len()),
        }
    }
}
