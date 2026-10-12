use super::*;

mod arith;
mod arith_divmod;
mod arith_double;
mod array;
mod basic;
mod call;
mod closure;
mod compare;
mod field;
mod method;
mod order;
mod sequence;
mod table;
mod tfor;
mod tfor_ipairs;
mod upval;
use arith::*;
use arith_divmod::*;
use arith_double::*;
use array::*;
use basic::*;
use call::*;
use closure::*;
use compare::*;
use field::*;
use method::*;
use order::*;
use sequence::*;
use table::*;
use tfor::*;
pub(super) use upval::upval_table_read;
use upval::*;

/// One recorded op as the emit pass sees it: its index, its register
/// window (`off`, and `regs` with the constant operand's virtual
/// register appended when it has one) and its decoded instruction.
pub(super) struct OpCx<'r> {
    pub(super) i: usize,
    pub(super) rop: &'r RecordedOp,
    pub(super) vregs: VRegs,
    pub(super) rc_const: Option<i64>,
    pub(super) off: usize,
    pub(super) regs: &'r [Variable],
    pub(super) ins: Inst,
    pub(super) op: Op,
    pub(super) max_stack: usize,
}

impl OpCx<'_> {
    /// The kind of an operand register, the virtual ones included.
    pub(super) fn kind(&self, current_kinds: &[RegKind], r: u32) -> RegKind {
        match (r as usize).checked_sub(self.max_stack) {
            Some(j) if j < NVIRT && self.vregs[j].is_some() => {
                vsrc_kind(self.vregs[j].expect("checked"))
            }
            _ => k_op(current_kinds, self.off as u32 + r),
        }
    }
}

/// Lowers one op of the body; `None` when the trace cannot be compiled.
pub(super) fn emit_op<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>) -> Option<()> {
    match oc.op {
        Op::Jmp
        | Op::Move
        | Op::LoadI
        | Op::LoadF
        | Op::LoadNil
        | Op::LoadK
        | Op::LoadFalse
        | Op::LoadTrue
        | Op::LFalseSkip
        | Op::LTrueSkip
        | Op::Not => emit_basic_op(lw, pl, oc),
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => emit_float_arith_op(lw, pl, oc),
        Op::IDiv
        | Op::Mod
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr
        | Op::Unm
        | Op::BNot => emit_int_arith_op(lw, pl, oc),
        Op::EqK => emit_eqk_op(lw, pl, oc),
        Op::Test | Op::TestSet => emit_test_op(lw, pl, oc),
        Op::Lt | Op::Le | Op::Eq => emit_order_op(lw, pl, oc),
        Op::NewTable | Op::GetI | Op::GetTable => emit_table_new_get_op(lw, pl, oc),
        Op::SetField | Op::SetI | Op::SetTable => emit_table_set_op(lw, pl, oc),
        Op::GetField => emit_get_field_op(lw, pl, oc),
        Op::SelfOp => emit_self_op(lw, pl, oc),
        Op::GetTabUp => emit_get_tab_up_op(lw, pl, oc),
        Op::SetList | Op::Len | Op::Concat => emit_sequence_op(lw, pl, oc),
        Op::Closure | Op::Close | Op::JmpClose | Op::JmpCloseBack | Op::GetUpval => {
            emit_closure_op(lw, pl, oc)
        }
        Op::Call | Op::Return0 | Op::Return1 | Op::Return | Op::Vararg => emit_call_op(lw, pl, oc),
        Op::TForCall | Op::TForCall53 | Op::TForCall55 => emit_tfor_call_op(lw, pl, oc),
        // generic-for prep is the leading pc-bump
        // before body_top. Recorder enters at body_top, so this
        // arm is defensive only — pre-emit pass bails before we
        // reach it.
        Op::TForPrep | Op::TForPrep53 | Op::TForPrep55 => {
            unreachable!("Op::TForPrep bailed in pre-emit pass")
        }
        // TForLoop is the trace's terminator; tail
        // emit handles the side-exit + back-edge.
        Op::TForLoop | Op::TForLoop53 | Op::TForLoop55 => {
            unreachable!("Op::TForLoop only appears at effective_end")
        }
        _ => unreachable!("non-whitelisted op rejected in pre-emit pass"),
    }
}
