//! `CLOSURE` in PUC 5.1 bytecode: the upvalues are named by the
//! pseudo-instructions that follow it.

use super::{I51, OP_GETUPVAL, OP_MOVE};
use crate::vm::dump::puc::lower::{Lowering, RawProto, enc_abx};
use crate::vm::isa::Op;

/// Lowers `CLOSURE` and the pseudo-instructions that name its upvalues;
/// returns the pc of the last of them.
pub(super) fn lower_closure(
    lw: &mut Lowering,
    protos: &mut [RawProto],
    closed: &mut [bool],
    code: &[u32],
    pc: usize,
    i: I51,
) -> Result<usize, String> {
    let idx = i.bx() as usize;
    let Some(child) = protos.get_mut(idx) else {
        return Err(lw.err(format_args!("CLOSURE of missing function {idx}")));
    };
    if std::mem::replace(&mut closed[idx], true) {
        return Err(lw.err(format_args!("function {idx} instantiated twice")));
    }
    for u in 1..child.upvals.len() {
        let Some(&w) = code.get(pc + u) else {
            return Err(lw.err("CLOSURE without its upvalue pseudo-instructions"));
        };
        let p = I51::decode(w);
        let (in_stack, index) = match p.op {
            OP_MOVE => (true, lw.r(p.b)?),
            OP_GETUPVAL => (false, p.b + 1),
            op => {
                return Err(lw.err(format_args!(
                    "CLOSURE upvalue pseudo-instruction has opcode {op}"
                )));
            }
        };
        let index = u8::try_from(index)
            .map_err(|_| lw.err(format_args!("upvalue index {index} past 255")))?;
        child.upvals[u].in_stack = in_stack;
        child.upvals[u].index = index;
    }
    let n_pseudo = child.upvals.len() - 1;
    let a = lw.r(i.a)?;
    lw.emit(enc_abx(Op::Closure, a, idx as u32)?);
    Ok(pc + n_pseudo)
}
