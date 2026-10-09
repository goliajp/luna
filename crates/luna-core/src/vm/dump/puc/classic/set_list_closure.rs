//! `SETLIST` and `CLOSURE` of 5.2 / 5.3 bytecode.

use super::*;

/// Lowers `SETLIST` and returns the pc of its last word (the `EXTRAARG`
/// when `C = 0` takes the block number from it).
pub(super) fn lower_set_list(
    lw: &mut Lowering,
    ops: &[Kind],
    code: &[u32],
    mut pc: usize,
    i: I,
) -> Result<usize, String> {
    let block = if i.c == 0 {
        pc += 1;
        match code.get(pc) {
            Some(&w) if kind(ops, w) == Some(Kind::ExtraArg) => ax(w),
            _ => return Err(lw.err("SETLIST without its EXTRAARG")),
        }
    } else {
        i.c
    };
    if block == 0 {
        return Err(lw.err("SETLIST block number 0"));
    }
    let a = lw.run(i.a, i.b + 1)?;
    lw.set_list(a, i.b, (block as u64 - 1) * FIELDS_PER_FLUSH)?;
    Ok(pc)
}

pub(super) fn lower_closure(
    lw: &mut Lowering,
    protos: &mut [RawProto],
    closed: &mut [bool],
    i: I,
) -> Result<(), String> {
    let idx = i.bx() as usize;
    let Some(child) = protos.get_mut(idx) else {
        return Err(lw.err(format_args!("CLOSURE of missing function {idx}")));
    };
    if std::mem::replace(&mut closed[idx], true) {
        return Err(lw.err(format_args!("function {idx} instantiated twice")));
    }
    for u in child.upvals.iter_mut().filter(|u| u.in_stack) {
        let r = lw.r(u.index as u32)?;
        // `r` is at most 255: `Lowering::reg_at` refuses more.
        u.index = r as u8;
    }
    let a = lw.r(i.a)?;
    lw.emit(enc_abx(Op::Closure, a, idx as u32)?);
    Ok(())
}
