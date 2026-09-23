//! Bytecode verifier for loaded binary chunks.
//!
//! luna's dispatch loop, its JITs and the AOT compiler read registers,
//! constants, upvalues and instructions without bounds checks, trusting the
//! invariants its own compiler establishes. A binary chunk is outside that
//! trust: luna's own dump format carries raw instruction words, and the PUC
//! translators lower whatever instruction stream the chunk holds. Every
//! chunk therefore passes through [`verify`] after it is read and before it
//! reaches the VM, whichever format it came in.
//!
//! The verifier knows only the `Proto` / instruction invariants below. Each
//! is what some part of the VM assumes; a chunk that breaks one is refused
//! with a `bad binary format (...)` load error.
//!
//! Per function:
//! - the code is non-empty (the first fetch is unchecked);
//! - `num_params <= max_stack` (frame entry clears `base + num_params ..
//!   base + max_stack` unchecked), and `num_params < max_stack` when the 5.1
//!   `arg` local is filled at entry;
//! - line info is empty (stripped) or has one entry per instruction;
//! - each upvalue descriptor names a register below the parent's
//!   `max_stack` (`in_stack`) or an upvalue of the parent.
//!
//! Per instruction:
//! - the opcode is one luna defines (`Inst::op` transmutes the raw byte);
//! - every register it reads or writes, including the runs implied by
//!   `Call`, `Return`, `LoadNil`, `Concat`, `SetList`, `Vararg` and the loop
//!   ops, lies below `max_stack`;
//! - constant, upvalue and child-function indices are in range;
//! - every successor (fall-through, jump target, the slot after a skipped
//!   instruction) lies inside the code, so control never runs off its end;
//! - `ExtraArg` follows exactly `LoadKx` and `SetList` with `k` set, and is
//!   never itself executed (no edge lands on it);
//! - loops are paired as luna's compiler lays them out: `ForPrep` skips to
//!   its `ForLoop`, whose back edge returns to the instruction after the
//!   `ForPrep`; `TForPrep` skips to its `TForCall`, which is followed by its
//!   `TForLoop`, whose back edge returns to the instruction after the
//!   `TForPrep`; each pair on the same base register. No `ForLoop` without
//!   its `ForPrep`, and every `TForLoop` directly after a `TForCall` on the
//!   same base (5.1–5.3 chunks enter a generic loop by a `Jmp` to the
//!   `TForCall`, with no `TForPrep`). The method JIT, the trace JIT's exits
//!   and the dispatcher's `TForLoop` resume read these neighbours;
//! - a conditional-skip instruction (`Eq`, `Lt`, `Le`, `EqK`, `Test`,
//!   `TestSet`) is followed by the `Jmp` it skips (both JITs lower the pair
//!   as one branch);
//! - an instruction taking a variable number of values from the stack top
//!   (`Call`/`TailCall` with `B = 0`, `Return` with `B = 0`, `SetList` with
//!   `B = 0`) directly follows the instruction that set that top (`Call`
//!   with `C = 0`, `TailCall`, `Vararg` with `C = 0`), is reached only from
//!   it, and reads from at or below the first value it produced — otherwise
//!   `top - A` underflows.
//!
//! Not verified: the *values* registers hold at run time. `SetList` expects
//! a table in `R[A]` and `ForLoop` expects the numbers `ForPrep` stored; the
//! debug library can change both from plain source code, so they are the
//! interpreter's to check, not the loader's.

mod operands;

use crate::runtime::function::Proto;
use crate::vm::isa::{Inst, NUM_OPS, Op};

/// Check `proto` and every function nested in it; `Err` carries the
/// complete load-error message.
pub(super) fn verify(proto: &Proto) -> Result<(), String> {
    verify_proto(proto, None).map_err(|e| format!("bad binary format ({e})"))
}

fn verify_proto(p: &Proto, parent: Option<&Proto>) -> Result<(), String> {
    check_header(p, parent).map_err(|e| format!("{}: {e}", describe(p)))?;
    let ops = decode_ops(p)?;
    let entered = Checker { p, ops: &ops }.check_code()?;
    Checker { p, ops: &ops }.check_open_top(&entered)?;
    for child in p.protos.iter() {
        verify_proto(child, Some(p))?;
    }
    Ok(())
}

/// `function at line N` / `main function`, to place an error.
fn describe(p: &Proto) -> String {
    if p.line_defined == 0 {
        "main function".to_string()
    } else {
        format!("function at line {}", p.line_defined)
    }
}

fn check_header(p: &Proto, parent: Option<&Proto>) -> Result<(), String> {
    if p.code.is_empty() {
        return Err("no instructions".to_string());
    }
    if p.num_params > p.max_stack {
        return Err(format!(
            "{} parameters exceed stack size {}",
            p.num_params, p.max_stack
        ));
    }
    if p.has_compat_vararg_arg && p.num_params >= p.max_stack {
        return Err(format!(
            "no register for 'arg' after {} parameters (stack size {})",
            p.num_params, p.max_stack
        ));
    }
    if !p.lines.is_empty() && p.lines.len() != p.code.len() {
        return Err(format!(
            "{} line entries for {} instructions",
            p.lines.len(),
            p.code.len()
        ));
    }
    let Some(parent) = parent else {
        return Ok(());
    };
    for (i, u) in p.upvals.iter().enumerate() {
        let (limit, what) = if u.in_stack {
            (parent.max_stack as usize, "register")
        } else {
            (parent.upvals.len(), "enclosing upvalue")
        };
        if u.index as usize >= limit {
            return Err(format!(
                "upvalue {} captures {what} {} out of range (limit {limit})",
                i + 1,
                u.index
            ));
        }
    }
    Ok(())
}

/// Decode every opcode, refusing a byte that names no `Op`.
fn decode_ops(p: &Proto) -> Result<Vec<Op>, String> {
    p.code
        .iter()
        .enumerate()
        .map(|(pc, inst)| {
            let raw = inst.0 & 0x7F;
            if raw as usize >= NUM_OPS {
                return Err(format!(
                    "{}, instruction {}: invalid opcode {raw}",
                    describe(p),
                    pc + 1
                ));
            }
            Ok(inst.op())
        })
        .collect()
}

/// `entered[pc]`: some edge other than fall-through from `pc - 1` reaches
/// `pc` (a jump, a loop edge, or a skip over `pc - 1`).
type Entered = Vec<bool>;

struct Checker<'a> {
    p: &'a Proto,
    ops: &'a [Op],
}

/// At most three successors per instruction.
struct Succ {
    fall: Option<usize>,
    other: [Option<i64>; 2],
}

impl Checker<'_> {
    fn err(&self, pc: usize, msg: String) -> String {
        format!(
            "{}, instruction {} ({:?}): {msg}",
            describe(self.p),
            pc + 1,
            self.ops[pc]
        )
    }

    fn max(&self) -> u32 {
        self.p.max_stack as u32
    }

    fn inst(&self, pc: usize) -> Inst {
        self.p.code[pc]
    }

    /// One pass: operands, successors, `ExtraArg` placement, loop pairing.
    fn check_code(&self) -> Result<Entered, String> {
        let n = self.ops.len();
        let mut entered = vec![false; n];
        if self.ops[0] == Op::ExtraArg {
            return Err(self.err(0, "executed as an instruction".to_string()));
        }
        // loop ends claimed by their prep
        let mut claimed = vec![false; n];
        for pc in 0..n {
            if self.ops[pc] == Op::ExtraArg {
                continue;
            }
            self.check_operands(pc)?;
            if let Some(end) = self.check_pairing(pc)? {
                claimed[end] = true;
            }
            let succ = self.successors(pc);
            if let Some(s) = succ.fall {
                self.check_target(pc, s as i64, "falls off the end of the code")?;
            }
            for s in succ.other.into_iter().flatten() {
                self.check_target(pc, s, "jumps outside the code")?;
                entered[s as usize] = true;
            }
        }
        for pc in 0..n {
            if self.ops[pc] == Op::ForLoop && !claimed[pc] {
                return Err(self.err(pc, "not paired with a ForPrep".to_string()));
            }
            let after_call = pc.checked_sub(1).is_some_and(|q| {
                self.ops[q] == Op::TForCall && self.inst(q).a() == self.inst(pc).a()
            });
            if self.ops[pc] == Op::TForLoop && !after_call {
                return Err(self.err(pc, "not preceded by its TForCall".to_string()));
            }
        }
        Ok(entered)
    }

    fn check_target(&self, pc: usize, s: i64, why: &str) -> Result<(), String> {
        if s < 0 || s as usize >= self.ops.len() {
            return Err(self.err(pc, format!("{why} (target {})", s + 1)));
        }
        if self.ops[s as usize] == Op::ExtraArg {
            return Err(self.err(
                pc,
                format!(
                    "control reaches the extra argument at instruction {}",
                    s + 1
                ),
            ));
        }
        Ok(())
    }

    fn successors(&self, pc: usize) -> Succ {
        let i = self.inst(pc);
        let pc_i = pc as i64;
        let next = pc + 1;
        let (fall, other) = match self.ops[pc] {
            Op::Return | Op::Return0 | Op::Return1 => (None, [None, None]),
            Op::Jmp => (None, [Some(pc_i + 1 + i.sj() as i64), None]),
            // the extra argument at pc + 1 is consumed, not executed
            Op::LoadKx => (None, [Some(pc_i + 2), None]),
            Op::SetList if i.k() => (None, [Some(pc_i + 2), None]),
            Op::Eq | Op::Lt | Op::Le | Op::EqK | Op::Test | Op::TestSet | Op::LFalseSkip => {
                (Some(next), [Some(pc_i + 2), None])
            }
            // 5.1–5.3 enter the loop at its ForLoop; 5.4+ skip past it
            Op::ForPrep => {
                let loop_pc = pc_i + i.bx() as i64;
                (Some(next), [Some(loop_pc), Some(loop_pc + 1)])
            }
            Op::ForLoop | Op::TForLoop => (Some(next), [Some(pc_i + 1 - i.bx() as i64), None]),
            Op::TForPrep => (None, [Some(pc_i + 1 + i.bx() as i64), None]),
            _ => (Some(next), [None, None]),
        };
        Succ { fall, other }
    }

    /// Loops as luna's compiler lays them out, and the `Jmp` after a
    /// conditional skip (see the module docs). Returns the loop end a prep
    /// claims.
    fn check_pairing(&self, pc: usize) -> Result<Option<usize>, String> {
        let i = self.inst(pc);
        let a = i.a();
        match self.ops[pc] {
            Op::ForPrep => {
                let lp = pc + i.bx() as usize;
                let paired = self.ops.get(lp) == Some(&Op::ForLoop) && {
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
            Op::TForPrep => {
                let call = pc + 1 + i.bx() as usize;
                let paired = self.ops.get(call) == Some(&Op::TForCall)
                    && self.inst(call).a() == a
                    && self.ops.get(call + 1) == Some(&Op::TForLoop)
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
            Op::TForCall => {
                let paired =
                    self.ops.get(pc + 1) == Some(&Op::TForLoop) && self.inst(pc + 1).a() == a;
                if !paired {
                    return Err(self.err(pc, "not followed by its TForLoop".to_string()));
                }
                Ok(None)
            }
            Op::Eq | Op::Lt | Op::Le | Op::EqK | Op::Test | Op::TestSet => {
                if self.ops.get(pc + 1) != Some(&Op::Jmp) {
                    return Err(self.err(pc, "not followed by a Jmp".to_string()));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// Instructions reading the stack top must directly follow the
    /// instruction that set it (see the module docs).
    fn check_open_top(&self, entered: &Entered) -> Result<(), String> {
        for pc in 0..self.ops.len() {
            let i = self.inst(pc);
            // the lowest register the consumer reads up from
            let floor = match self.ops[pc] {
                Op::Call | Op::TailCall if i.b() == 0 => i.a() + 1,
                Op::SetList if i.b() == 0 => i.a() + 1,
                Op::Return if i.b() == 0 => i.a(),
                _ => continue,
            };
            let producer = pc.checked_sub(1).and_then(|q| {
                let pi = self.inst(q);
                match self.ops[q] {
                    Op::Call if pi.c() == 0 => Some(pi.a()),
                    Op::Vararg if pi.c() == 0 => Some(pi.a()),
                    Op::TailCall => Some(pi.a()),
                    _ => None,
                }
            });
            match producer {
                Some(first) if !entered[pc] && first >= floor => {}
                _ => {
                    return Err(self.err(
                        pc,
                        "takes values from a stack top no instruction set".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}
