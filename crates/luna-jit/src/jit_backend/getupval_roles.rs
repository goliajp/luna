//! Which `GetUpval` reads of a chunk feed arithmetic (a value) and which
//! only name the function itself (a self-call marker).

use luna_core::vm::isa::{Inst, Op};

/// classify every `Op::GetUpval` in `code` as either the
/// existing **SelfMarker** role (the loaded value is used only as a
/// `Op::Call` func slot — lowered as a direct cranelift call
/// without ever materialising the upvalue) or the new **ValueRead**
/// role (the loaded value flows into arith / cmp / unary, so we need
/// the real value at runtime via `luna_jit_upval_get` and the pre53
/// path's "default Float" assumption is sound).
///
/// Algorithm: from each `GetUpval R[A]` at PC X, walk forward up to 8
/// instructions. The FIRST event for R[A] decides:
/// - `Op::Call` with `a == A` → SelfMarker
/// - arith / cmp / unary reading R[A] → ValueRead (the operand is
///   provably numeric: an interpreter would raise on a non-numeric
///   upvalue, so we won't miscompile silent data)
/// - any other op writing R[A] (or window end) → default SelfMarker,
///   which the rest of the scan handles via the existing arith-bail
///   on `self_upval`. Cases like `function () return x end` (Return1
///   reads R[A] without a numeric operator) stay in the default-bail
///   bucket because we can't assume the runtime type of `x`.
pub(super) fn determine_getupval_roles(code: &[Inst]) -> Vec<bool> {
    const WINDOW: usize = 8;
    let n = code.len();
    let mut roles = vec![false; n];
    for pc in 0..n {
        let ins = code[pc];
        if !matches!(ins.op(), Op::GetUpval) {
            continue;
        }
        let target_a = ins.a() as usize;
        let end = (pc + 1 + WINDOW).min(n);
        for q in (pc + 1)..end {
            let q_ins = code[q];
            // Op::Call with R[A] as func slot — confirmed SelfMarker.
            if matches!(q_ins.op(), Op::Call) && q_ins.a() as usize == target_a {
                break;
            }
            // Arith / cmp / unary reading R[A] — confirmed ValueRead.
            if reads_register_a_arith(q_ins, target_a) {
                roles[pc] = true;
                break;
            }
            // R[A] overwritten before we confirmed either role — bail
            // to SelfMarker default (existing bail-on-arith-read in
            // the main scan handles it conservatively).
            if writes_register_a(q_ins, target_a) {
                break;
            }
        }
        // Window ended without an arith confirmation → roles[pc] stays
        // false (SelfMarker default).
    }
    roles
}

/// Helper for `determine_getupval_roles`: does `ins` *read* register
/// `target_a` via an arithmetic, comparison, or unary operator? These
/// are the ops whose interpreter semantics require a numeric operand
/// (otherwise PUC raises "attempt to perform arithmetic on a X
/// value"), so a JIT-side helper-fetch that interprets the upvalue
/// as Float is safe in pre53 dialects where Float is the only number
/// type.
fn reads_register_a_arith(ins: Inst, target_a: usize) -> bool {
    let b = ins.b() as usize;
    let c = ins.c() as usize;
    let a = ins.a() as usize;
    match ins.op() {
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Pow | Op::IDiv => {
            b == target_a || c == target_a
        }
        Op::Lt | Op::Le | Op::Eq => a == target_a || b == target_a,
        Op::Unm | Op::BNot | Op::Not => b == target_a,
        _ => false,
    }
}

/// Helper for `determine_getupval_roles`: does `ins` write to register
/// index `target_a`? Conservative — list every op that has a write
/// target including range-writers (LoadNil, ForPrep, ForLoop) where
/// `target_a` may fall inside the affected range.
fn writes_register_a(ins: Inst, target_a: usize) -> bool {
    let a = ins.a() as usize;
    match ins.op() {
        Op::LoadI
        | Op::LoadF
        | Op::LoadK
        | Op::LoadKx
        | Op::LoadFalse
        | Op::LFalseSkip
        | Op::LTrueSkip
        | Op::LoadTrue
        | Op::Move
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Mod
        | Op::Pow
        | Op::Div
        | Op::IDiv
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr
        | Op::Unm
        | Op::BNot
        | Op::Not
        | Op::Len
        | Op::Call
        | Op::GetUpval
        | Op::GetTabUp
        | Op::GetTable
        | Op::GetI
        | Op::GetField
        | Op::NewTable
        | Op::SelfOp => a == target_a,
        Op::Concat => {
            let (first, out) = ins.concat_operands();
            target_a == first as usize || target_a == out as usize
        }
        Op::LoadNil => target_a >= a && target_a <= a + ins.b() as usize,
        Op::ForPrep | Op::ForLoop | Op::ForPrep55 | Op::ForLoop55 => {
            let var = ins.op().for_layout().map_or(0, |l| l.var()) as usize;
            target_a >= a && target_a <= a + var
        }
        _ => false,
    }
}
