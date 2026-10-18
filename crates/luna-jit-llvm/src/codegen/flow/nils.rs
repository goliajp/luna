//! The registers holding nil before each op.

use super::*;

/// Which registers hold nil (rather than an integer) before each op, for a
/// level handing itself to the interpreter there: the interpreter then
/// sees them as they are. `None` at an op where a register is nil on one
/// way in and an integer on another (no level hands over there); such an
/// op among `at`, the self calls and the self tail calls, refuses the
/// function.
pub(in crate::codegen) fn nils_at(
    code: &[Inst],
    consumed_jmp: &[bool],
    regs: u32,
    num_params: u32,
    at: (&[bool], &[bool]),
) -> Option<Vec<Option<Vec<bool>>>> {
    // per register: 0 nil, 1 integer, 2 either
    let n = code.len();
    let mut entry: Vec<Option<Vec<u8>>> = vec![None; n];
    let mut start = vec![0u8; regs as usize];
    for r in start.iter_mut().take(num_params as usize) {
        *r = 1;
    }
    entry[0] = Some(start);
    let mut work = vec![0usize];
    while let Some(pc) = work.pop() {
        let Some(mut set) = entry[pc].clone() else {
            continue;
        };
        let ins = code[pc];
        match ins.op() {
            Op::LoadNil => {
                for r in ins.a()..=ins.a() + ins.b() {
                    set[r as usize] = 0;
                }
            }
            Op::Move => set[ins.a() as usize] = set[ins.b() as usize],
            Op::LoadI | Op::GetUpval | Op::Call => set[ins.a() as usize] = 1,
            op if is_arith(op) => set[ins.a() as usize] = 1,
            _ => {}
        }
        let succ: Vec<usize> = match ins.op() {
            Op::Return0 | Op::Return1 | Op::TailCall => Vec::new(),
            Op::Jmp if consumed_jmp[pc] => Vec::new(),
            Op::Jmp => vec![jmp_target(pc, ins)],
            op if is_compare(op) => {
                let mut v = vec![pc + 2];
                if let Some(j) = code.get(pc + 1) {
                    v.push(jmp_target(pc + 1, *j));
                }
                v
            }
            _ => vec![pc + 1],
        };
        for t in succ.into_iter().filter(|&t| t < n) {
            let merged = match &entry[t] {
                None => set.clone(),
                Some(old) => old
                    .iter()
                    .zip(&set)
                    .map(|(&a, &b)| if a == b { a } else { 2 })
                    .collect(),
            };
            if entry[t].as_ref() != Some(&merged) {
                entry[t] = Some(merged);
                work.push(t);
            }
        }
    }
    // a level may hand itself over only where every register is one or
    // the other; a self call must be such a place
    let mut out = vec![None; n];
    for pc in 0..n {
        let Some(set) = &entry[pc] else { continue };
        if set.contains(&2) {
            if at.0[pc] || at.1[pc] {
                return None;
            }
            continue;
        }
        out[pc] = Some(set.iter().map(|&s| s == 0).collect());
    }
    Some(out)
}
