//! Control flow: where each instruction goes on, and the loops paired
//! as luna's compiler lays them out (see the parent module's docs).

use super::{Checker, Succ};
use crate::vm::isa::Op;

impl Checker<'_> {
    pub(super) fn successors(&self, pc: usize) -> Succ {
        let i = self.inst(pc);
        let pc_i = pc as i64;
        let next = pc + 1;
        let (fall, other) = match self.ops[pc] {
            Op::Return | Op::Return0 | Op::Return1 => (None, [None, None]),
            Op::Jmp | Op::JmpClose | Op::JmpCloseBack => {
                (None, [Some(pc_i + 1 + i.jump_offset() as i64), None])
            }
            // the extra argument at pc + 1 is consumed, not executed
            Op::LoadKx => (None, [Some(pc_i + 2), None]),
            Op::SetList if i.k() => (None, [Some(pc_i + 2), None]),
            op if op.is_test() || op == Op::LFalseSkip => (Some(next), [Some(pc_i + 2), None]),
            // 5.1–5.3 enter the loop at its ForLoop; 5.4+ skip past it
            op if op.is_for_prep() => {
                let loop_pc = pc_i + i.bx() as i64;
                (Some(next), [Some(loop_pc), Some(loop_pc + 1)])
            }
            op if op.is_for_loop() || op.is_tfor_loop() => {
                (Some(next), [Some(pc_i + 1 - i.bx() as i64), None])
            }
            op if op.is_tfor_prep() => (None, [Some(pc_i + 1 + i.bx() as i64), None]),
            _ => (Some(next), [None, None]),
        };
        Succ { fall, other }
    }

    /// Loops as luna's compiler lays them out, and the `Jmp` after a
    /// conditional skip (see the module docs). Returns the loop end a prep
    /// claims.
    pub(super) fn check_pairing(&self, pc: usize) -> Result<Option<usize>, String> {
        let i = self.inst(pc);
        let a = i.a();
        match self.ops[pc] {
            op if op.is_for_prep() => {
                let lp = pc + i.bx() as usize;
                let want = op.for_layout().map(|l| l.ops().2);
                let paired = self.ops.get(lp).copied() == want && {
                    let l = self.inst(lp);
                    l.a() == a && lp as i64 + 1 - l.bx() as i64 == pc as i64 + 1
                };
                if !paired {
                    return Err(
                        self.err(pc, format!("no matching ForLoop at instruction {}", lp + 1))
                    );
                }
                Ok(Some(lp))
            }
            op if op.is_tfor_prep() => {
                let (_, call_op, loop_op) = op.for_layout().expect("a loop op").ops();
                let call = pc + 1 + i.bx() as usize;
                let paired = self.ops.get(call) == Some(&call_op)
                    && self.inst(call).a() == a
                    && self.ops.get(call + 1) == Some(&loop_op)
                    && {
                        let l = self.inst(call + 1);
                        l.a() == a && call as i64 + 2 - l.bx() as i64 == pc as i64 + 1
                    };
                if !paired {
                    return Err(self.err(
                        pc,
                        format!("no matching TForCall/TForLoop at instruction {}", call + 1),
                    ));
                }
                Ok(Some(call + 1))
            }
            op if op.is_tfor_call() => {
                let loop_op = op.for_layout().expect("a loop op").ops().2;
                if self.ops.get(pc + 1) != Some(&loop_op) || self.inst(pc + 1).a() != a {
                    return Err(self.err(pc, "not followed by its TForLoop".to_string()));
                }
                Ok(None)
            }
            op if op.is_test() => {
                if !matches!(
                    self.ops.get(pc + 1),
                    Some(Op::Jmp | Op::JmpClose | Op::JmpCloseBack)
                ) {
                    return Err(self.err(pc, "not followed by a Jmp".to_string()));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
