//! Control-flow passes over a whitelisted chunk.

use super::plan::jmp_target;
use crate::operands::{is_arith, is_compare, operand_regs};
use luna_core::vm::isa::{Inst, Op};

/// Pass 2: reach analysis from PC 0. A worklist trace
/// following the control-flow edges; terminators have no
/// successor.
pub(super) fn reachable(code: &[Inst], consumed_jmp: &[bool]) -> Vec<bool> {
    let n = code.len();
    let mut reachable = vec![false; n];
    let mut worklist = vec![0usize];
    while let Some(pc) = worklist.pop() {
        if pc >= n || reachable[pc] {
            continue;
        }
        reachable[pc] = true;
        let ins = code[pc];
        match ins.op() {
            Op::Return0 | Op::Return1 | Op::TailCall => {} // terminator, no successor
            Op::Jmp => {
                if consumed_jmp[pc] {
                    // Consumed by the preceding Lt|Le|Eq; the
                    // edges from this Jmp are folded into the
                    // comparison's condbr. Visiting it again
                    // here would mark it reachable as a
                    // standalone op, which is wrong (the emit
                    // loop skips consumed Jmps).
                    continue;
                }
                worklist.push(jmp_target(pc, ins));
            }
            op if is_compare(op) => {
                // The peer Jmp at pc+1 supplies the false-edge
                // target; pc+2 is the true-edge (skip-next) fall.
                worklist.push(pc + 2);
                if let Some(jmp) = code.get(pc + 1) {
                    worklist.push(jmp_target(pc + 1, *jmp));
                }
            }
            _ => worklist.push(pc + 1),
        }
    }
    reachable
}

/// Whether a `Return` is reachable from PC 0 without passing a
/// self-recursive call. A compiled self call recurses on the native stack,
/// past the interpreter's depth limit: a function with no such path (no
/// base case, `local function f() return f() + 1 end`) would overflow the
/// process stack instead of raising Lua's "stack overflow".
pub(super) fn has_base_case(
    code: &[Inst],
    consumed_jmp: &[bool],
    self_call_pcs: &[bool],
    tail_call_pcs: &[bool],
) -> bool {
    let n = code.len();
    let mut seen = vec![false; n];
    let mut worklist = vec![0usize];
    while let Some(pc) = worklist.pop() {
        if pc >= n || seen[pc] {
            continue;
        }
        seen[pc] = true;
        if self_call_pcs[pc] || tail_call_pcs[pc] {
            continue;
        }
        let ins = code[pc];
        match ins.op() {
            Op::Return0 | Op::Return1 => return true,
            Op::TailCall => {}
            Op::Jmp if consumed_jmp[pc] => {}
            Op::Jmp => worklist.push(jmp_target(pc, ins)),
            op if is_compare(op) => {
                worklist.push(pc + 2);
                if let Some(jmp) = code.get(pc + 1) {
                    worklist.push(jmp_target(pc + 1, *jmp));
                }
            }
            _ => worklist.push(pc + 1),
        }
    }
    false
}

/// Pass 3: returns_one analysis. Every reachable Return* must
/// agree on shape (Return0 or Return1) so the dispatcher
/// contract has a single answer. Op::TailCall is treated as
/// Return1 (it returns the called function's result = one value).
pub(super) fn returns_one(code: &[Inst], reachable: &[bool]) -> Option<bool> {
    let mut found: Option<bool> = None;
    for (pc, ins) in code.iter().enumerate() {
        if !reachable[pc] {
            continue;
        }
        match ins.op() {
            Op::Return0 => match found {
                Some(true) => return None,
                _ => found = Some(false),
            },
            Op::Return1 | Op::TailCall => match found {
                Some(false) => return None,
                _ => found = Some(true),
            },
            _ => {}
        }
    }
    found
}

/// Pass 4: BB starts. PC 0 always; jump targets; every PC
/// immediately after a terminator; the fall-through PC after
/// a comparison (pc+2 — the consumed Jmp at pc+1 is folded).
pub(super) fn bb_starts(code: &[Inst], reachable: &[bool], consumed_jmp: &[bool]) -> Vec<bool> {
    let n = code.len();
    let mut bb_starts = vec![false; n];
    bb_starts[0] = true;
    for (pc, ins) in code.iter().enumerate() {
        if !reachable[pc] {
            continue;
        }
        match ins.op() {
            op if is_compare(op) => {
                if pc + 2 < n {
                    bb_starts[pc + 2] = true;
                }
                if let Some(jmp) = code.get(pc + 1) {
                    let tgt = jmp_target(pc + 1, *jmp);
                    if tgt < n {
                        bb_starts[tgt] = true;
                    }
                }
            }
            Op::Jmp if !consumed_jmp[pc] => {
                let tgt = jmp_target(pc, *ins);
                if tgt < n {
                    bb_starts[tgt] = true;
                }
                if pc + 1 < n {
                    bb_starts[pc + 1] = true;
                }
            }
            Op::Return0 | Op::Return1 | Op::TailCall if pc + 1 < n => {
                bb_starts[pc + 1] = true;
            }
            _ => {}
        }
    }
    bb_starts
}

/// Register-bounds sanity check on every reachable op that
/// names a slot. `proto.max_stack` is the upper bound the
/// parser guarantees; an out-of-range A would write past the
/// alloca, so bail rather than corrupt memory.
pub(super) fn registers_in_bounds(code: &[Inst], reachable: &[bool], regs: u32) -> bool {
    for (pc, ins) in code.iter().enumerate() {
        if !reachable[pc] {
            continue;
        }
        let operands = operand_regs(*ins).into_iter().max().unwrap_or(0);
        let max_slot = match ins.op() {
            Op::LoadI | Op::Move | Op::Return1 | Op::GetUpval => ins.a(),
            Op::LoadNil => ins.a() + ins.b(),
            op if is_arith(op) => ins.a().max(operands),
            op if is_compare(op) => operands,
            Op::Call | Op::TailCall => {
                // Op::Call/TailCall read R[A] (function) and
                // R[A+1..A+nargs] (args); A is the max slot.
                let nargs = ins.b().saturating_sub(1);
                if nargs == 0 { ins.a() } else { ins.a() + nargs }
            }
            _ => 0,
        };
        if max_slot >= regs {
            return false;
        }
    }
    true
}

/// nil is kept as the payload 0, which the integer ops take for the
/// integer 0 (`x == nil` held for `x = 0`, `return x` gave 0): no
/// reachable op may read a register a LoadNil can reach
pub(super) fn reads_no_nil(code: &[Inst], reachable: &[bool], regs: u32) -> bool {
    let mut maybe_nil = vec![false; regs as usize];
    let mut changed = true;
    while changed {
        changed = false;
        for (pc, ins) in code.iter().enumerate() {
            if !reachable[pc] {
                continue;
            }
            let dst = match ins.op() {
                Op::LoadNil => ins.a()..=ins.a() + ins.b(),
                Op::Move if maybe_nil[ins.b() as usize] => ins.a()..=ins.a(),
                _ => continue,
            };
            for r in dst {
                if !maybe_nil[r as usize] {
                    maybe_nil[r as usize] = true;
                    changed = true;
                }
            }
        }
    }
    for (pc, ins) in code.iter().enumerate() {
        if !reachable[pc] {
            continue;
        }
        let reads: Vec<u32> = match ins.op() {
            op if is_arith(op) || is_compare(op) => operand_regs(*ins),
            Op::Return1 => vec![ins.a()],
            Op::Call | Op::TailCall => (1..ins.b()).map(|off| ins.a() + off).collect(),
            _ => continue,
        };
        if reads.iter().any(|&r| maybe_nil[r as usize]) {
            return false;
        }
    }
    true
}
