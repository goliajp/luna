//! Which `GetUpval`s of a function load the function itself for a
//! self-recursive call and which read an upvalue's value.

use crate::operands::{is_arith, is_compare, operand_regs};
use luna_core::vm::isa::{Inst, Op};

/// Classify every `Op::GetUpval` in `code` as
/// either a SelfMarker (its destination register is consumed only as
/// the function slot of a subsequent `Op::Call`) or a ValueRead (the
/// register is consumed as a real value — arith, comparison, return,
/// etc.). Mirrors Cranelift's `determine_getupval_roles` algorithm.
///
/// The classifier walks an 8-PC lookahead from each `GetUpval`,
/// tracking `Move`-carrying tags on each register. If the destination
/// register reaches an `Op::Call` whose function-slot register is the
/// tagged one, it's SelfMarker. Any other consuming op flips it to
/// ValueRead. The lookahead is bounded to keep the scanner O(N).
pub(crate) fn determine_getupval_roles(code: &[Inst]) -> Vec<bool> {
    let n = code.len();
    let mut roles = vec![false; n];
    // 8-PC lookahead window, matching Cranelift S2c.C scanner.
    const LOOKAHEAD: usize = 8;
    for (pc, ins) in code.iter().enumerate() {
        if !matches!(ins.op(), Op::GetUpval) {
            continue;
        }
        let target_a = ins.a() as usize;
        // Per-register "carries the marker" map for this lookahead.
        let max_reg = code
            .iter()
            .map(|i| i.a().max(i.b()).max(i.c()))
            .max()
            .unwrap_or(0) as usize
            + 1;
        let mut tagged: Vec<bool> = vec![false; max_reg.max(target_a + 1)];
        tagged[target_a] = true;
        let end = (pc + 1 + LOOKAHEAD).min(n);
        let mut value_read = false;
        for q in (pc + 1)..end {
            let q_ins = code[q];
            // Op::Call with R[A] as the function slot — confirmed SelfMarker.
            if matches!(q_ins.op(), Op::Call)
                && tagged.get(q_ins.a() as usize).copied().unwrap_or(false)
            {
                // SelfMarker: leave roles[pc] = false.
                value_read = false;
                break;
            }
            // Move from tagged src to dst: carry the tag.
            if matches!(q_ins.op(), Op::Move) {
                let src = q_ins.b() as usize;
                let dst = q_ins.a() as usize;
                if dst >= tagged.len() {
                    tagged.resize(dst + 1, false);
                }
                let carry = tagged.get(src).copied().unwrap_or(false);
                tagged[dst] = carry;
                continue;
            }
            // Any op that reads a tagged register in a non-Call /
            // non-Move context confirms ValueRead.
            if uses_register_as_value(&q_ins, &tagged) {
                value_read = true;
                break;
            }
            // Any write to a tagged register clears the tag (the
            // dest no longer carries the marker).
            if let Some(write_reg) = primary_write_reg(&q_ins)
                && let Some(slot) = tagged.get_mut(write_reg as usize)
            {
                *slot = false;
            }
        }
        roles[pc] = value_read;
    }
    roles
}

/// Helper for `determine_getupval_roles`: returns `true` if `ins`
/// reads any register currently flagged in `tagged` as a value-use
/// (NOT as the function slot of an `Op::Call`).
fn uses_register_as_value(ins: &Inst, tagged: &[bool]) -> bool {
    let is_tagged = |idx: u32| tagged.get(idx as usize).copied().unwrap_or(false);
    match ins.op() {
        Op::Return1 => is_tagged(ins.a()),
        op if is_arith(op) || is_compare(op) => operand_regs(*ins).into_iter().any(is_tagged),
        // Op::Call / Op::TailCall args (R[A+1..A+nargs]) are value uses.
        Op::Call | Op::TailCall => {
            let nargs = ins.b().saturating_sub(1);
            for off in 1..=nargs {
                if is_tagged(ins.a() + off) {
                    return true;
                }
            }
            false
        }
        // Move is handled separately by the caller (it carries the
        // tag rather than confirming ValueRead).
        Op::Move => false,
        _ => false,
    }
}

/// Helper for `determine_getupval_roles`: returns the primary write
/// destination register for the supported op set. Used to clear the
/// tagged marker when a register is overwritten.
fn primary_write_reg(ins: &Inst) -> Option<u32> {
    match ins.op() {
        Op::LoadI | Op::LoadNil | Op::Move | Op::GetUpval | Op::Call => Some(ins.a()),
        op if is_arith(op) => Some(ins.a()),
        _ => None,
    }
}
