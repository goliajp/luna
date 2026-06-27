//! v2.1 Phase 1K.D — LLVM int-chunk codegen entry.
//!
//! Phase 1K.D.5 stub: returns `None` for every shape so the trait
//! impl reports `CompileResult::Skipped` and the interpreter handles
//! the chunk unchanged.
//!
//! Phase 1K.D.6 will light up `Op::LoadNil` end-to-end; Phase 1K.D.7
//! extends to a 3-op chunk (`LoadNil` + `LoadK` + `Move`). Phase
//! 1K.E grows out to the full Cranelift int-chunk whitelist.

use luna_core::jit::{CompileResult, JitStorage};
use luna_core::runtime::{Gc, function::Proto};

/// Try to lower `proto` to native code via LLVM. Returns
/// `Some(CompileResult::Compiled { .. })` on success, `None` on bail.
/// `pre53` distinguishes Lua 5.1-5.3 `for` loop dialect from 5.4+; the
/// stub ignores it because no op is recognised yet.
#[allow(unused_variables)]
pub(crate) fn try_compile_int_chunk(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
) -> Option<CompileResult> {
    // Phase 1K.D.5 stub: every shape bails. Phase 1K.D.6 will pattern-
    // match `proto.code` against `[Op::LoadNil(R0, 0), Op::Return0]`
    // (or `Return1(R0)`) and emit an LLVM IR fn that returns 0.
    None
}
