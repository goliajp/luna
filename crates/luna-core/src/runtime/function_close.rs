//! Which returns of a function have something to close (PUC `luaK_finish`
//! turning `OP_RETURN0` / `OP_RETURN1` into `OP_RETURN` with `k`).

use super::{Gc, Proto};
use crate::vm::isa::{Inst, Op};

/// Whether a return from a function with this code and these nested
/// functions may find something to close: a register a nested function
/// captures (an open upvalue) or a to-be-closed variable.
pub(crate) fn needs_close(code: &[Inst], protos: &[Gc<Proto>]) -> bool {
    protos.iter().any(|p| p.upvals.iter().any(|u| u.in_stack))
        || code.iter().any(|i| i.op() == Op::Tbc)
}

/// Set `k` on every `Return0` / `Return1` of a function that
/// [`needs_close`]: the interpreter's fast return looks for open upvalues
/// and to-be-closed slots only when `k` is set.
pub(crate) fn mark_closing_returns(code: &mut [Inst], protos: &[Gc<Proto>]) {
    if !needs_close(code, protos) {
        return;
    }
    for i in code.iter_mut() {
        if matches!(i.op(), Op::Return0 | Op::Return1) {
            *i = Inst::iabc(i.op(), i.a(), i.b(), i.c(), true);
        }
    }
}
