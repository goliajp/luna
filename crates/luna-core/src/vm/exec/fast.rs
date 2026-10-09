//! The interpreter's fast loop: the opcodes that never call, allocate or
//! run a metamethod execute here on locals, in a function of their own so
//! that its register allocation does not depend on the rest of the loop.

use super::num_double as dbl;
use super::*;
use crate::runtime::value::tag;
use call_fast::Returned;
use fast_arith::{
    arith_arm, arith_imm_arm, cold_path, put_int, raw_flt, raw_gc, raw_int, raw_tag, raw_truthy,
};
use index_fast::self_key;

mod arm_helpers;
mod arms_arith;
mod arms_call;
mod arms_cmp;
mod arms_load;
mod arms_loop;
mod flow;
mod step;
use arm_helpers::fast_arm_helper_macros;
use arms_arith::fast_arith_arms;
use arms_call::fast_call_arms;
use arms_cmp::fast_cmp_arms;
use arms_load::fast_load_arms;
use arms_loop::fast_loop_arms;
use flow::fast_flow_macros;
use step::fast_step_macros;

/// The running frame, as the loop head found it.
pub(super) struct Fast {
    /// the running frame, on top of `frames`
    pub(super) fr: *mut Frame,
    pub(super) trace_on: bool,
    pub(super) pre53: bool,
    pub(super) entry_depth: usize,
    /// false when some instruction needs the loop head's checks (a trap,
    /// a recording, or more trace heads than `heads` holds)
    pub(super) stay: bool,
    /// the pcs where a trace this function could enter starts
    pub(super) heads: [u32; crate::runtime::function::TRACE_HEADS_CAP],
}

/// Why the fast loop handed control back.
pub(super) enum FastExit {
    /// the frame state may have changed: back to the loop head
    Reload,
    /// an opcode the loop head's own match runs
    Slow(Inst),
}

impl Vm {
    /// The register window of a frame at `base`. It is valid while
    /// `self.stack` neither moves nor is written through a reference, and
    /// `push_frame` sized the stack to `base + max_stack`, so every
    /// register the frame's instructions name is inside it.
    #[inline(always)]
    fn regs_at(&mut self, base: u32) -> *mut Value {
        self.stack.as_mut_ptr().wrapping_add(base as usize)
    }

    /// Run instructions from `inst` (at `npc - 1`) until one needs the loop
    /// head. The frame's pc is `npc` on entry and is kept current. `WATCH`
    /// is false when `fx.stay` holds and `fx.heads` is empty: that loop then
    /// tests nothing per instruction. Without `WATCH`, `TRACE` is
    /// `fx.trace_on`, fixed for the loop so that it costs nothing. `DBL`
    /// is true for 5.1/5.2, whose integers stand for doubles (see
    /// [`super::num_double`]).
    #[inline(never)]
    pub(super) fn run_fast<const WATCH: bool, const TRACE: bool, const DBL: bool>(
        &mut self,
        fx: Fast,
        mut inst: Inst,
        mut npc: u32,
    ) -> Result<FastExit, LuaError> {
        let Fast {
            mut fr,
            trace_on: trace_rt,
            pre53,
            entry_depth,
            stay,
            heads,
        } = fx;
        let trace_on = if WATCH { trace_rt } else { TRACE };
        // From here the running frame's state lives in locals (PUC keeps
        // `pc`, `base` and `k` in registers the same way). An arm that only
        // reads and writes registers advances `npc` and stores it through
        // `fr`, which keeps the frame's pc current for errors and for
        // everything that reads it. An arm that may run Lua code, move the
        // stack or change frames goes through `resume!` or `reenter!`
        // afterwards; a metamethod call always pushes a continuation, which
        // sets `trap`, and then the loop head takes over (PUC `Protect`).
        // The loop keeps fetching here while no instruction needs the head's
        // checks: no trap, no recording and no compiled trace this function
        // could enter.
        // `fr` points into `self.frames`: it is taken again after anything
        // that can push a frame. The frame's other fields are read through
        // it where an arm needs them, which keeps them out of the loop's
        // registers.
        macro_rules! cl {
            () => {
                // SAFETY: `fr` is the running frame
                unsafe { (*fr).closure }
            };
        }
        macro_rules! base {
            () => {
                // SAFETY: as above
                unsafe { (*fr).base }
            };
        }
        let mut code = cl!().code;
        let mut kptr = cl!().consts;
        let base = base!();
        let mut regs = self.regs_at(base);
        fast_step_macros!($, self, fr, regs, npc, inst, code, trace_on, stay, heads);
        // `'frames` starts over on whatever frame is on top after a call or
        // return; the first pass runs the frame the loop head handed over
        let mut switched = false;
        'frames: loop {
            if switched {
                // whoever continued here set `fr` to the frame now on top
                // SAFETY: `fr` points at that frame
                let f = unsafe { &mut *fr };
                let cl = f.closure;
                npc = f.pc;
                let base = f.base;
                fr = f;
                code = cl.code;
                kptr = cl.consts;
                // stay only between frames with nothing to watch, so that
                // `stay` and `heads` hold for the whole loop
                if WATCH
                    || trace_on
                        && (self.jit.active_trace.is_some()
                            || cl.proto.trace_heads.get()[0]
                                != crate::runtime::function::TRACE_HEADS_NONE)
                {
                    // the loop head looks at this pc first
                    return Ok(FastExit::Reload);
                }
                regs = self.regs_at(base);
                inst = fetch!(npc);
                npc += 1;
            }
            switched = true;
            fast_flow_macros!($, self, fr, regs, npc, inst, code, kptr, trace_on, entry_depth, 'frames);
            fast_arm_helper_macros!($, self, fr, regs, npc, inst, code, kptr, trace_on, entry_depth, 'frames);
            loop {
                // the instruction now running
                let pc = npc - 1;
                fast_load_arms!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                fast_arith_arms!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                fast_cmp_arms!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                fast_loop_arms!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                fast_call_arms!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                match inst.op() {
                    Op::Move => op_move!(),
                    Op::LoadI => op_load_i!(),
                    Op::LoadF => op_load_f!(),
                    Op::LoadK => op_load_k!(),
                    Op::LoadFalse => op_load_false!(),
                    Op::LFalseSkip => op_l_false_skip!(),
                    Op::LoadTrue => op_load_true!(),
                    Op::LoadNil => op_load_nil!(),
                    Op::GetUpval => op_get_upval!(),
                    Op::SetUpval => op_set_upval!(),
                    Op::GetTabUp => op_get_tab_up!(),
                    Op::GetTable => get_arm!(regs.wrapping_add(inst.c() as usize), index_raw_at),
                    Op::GetField => op_get_field!(),
                    Op::GetI => op_get_i!(),
                    Op::SetTabUp => op_set_tab_up!(),
                    Op::SetTable => set_arm!(regs.wrapping_add(inst.b() as usize), newindex_raw_at),
                    Op::SetField => op_set_field!(),
                    Op::SetI => op_set_i!(),
                    Op::GetTableK => get_arm!(kptr.wrapping_add(inst.c() as usize), index_raw_at),
                    Op::SetTableK => {
                        set_arm!(kptr.wrapping_add(inst.b() as usize), newindex_raw_at)
                    }
                    Op::GetTabUpR => op_get_tab_up_r!(),
                    Op::GetGlobal => op_get_global!(),
                    Op::SetGlobal => op_set_global!(),
                    Op::SetTabUpR => op_set_tab_up_x!(regs.wrapping_add(inst.b() as usize)),
                    Op::SetTabUpK => op_set_tab_up_x!(kptr.wrapping_add(inst.b() as usize)),
                    Op::SelfOp => op_self_op!(),
                    Op::Add => op_add!(),
                    Op::Sub => op_sub!(),
                    Op::Mul => op_mul!(),
                    Op::Mod => op_mod!(),
                    Op::IDiv => op_i_div!(),
                    Op::Div => op_div!(),
                    Op::BAnd => op_b_and!(),
                    Op::BOr => op_b_or!(),
                    Op::BXor => op_b_xor!(),
                    Op::Shl => op_shl!(),
                    Op::Shr => op_shr!(),
                    Op::AddI => op_add_i!(),
                    Op::SubI => op_sub_i!(),
                    Op::AddK => op_add_k!(),
                    Op::SubK => op_sub_k!(),
                    Op::MulK => op_mul_k!(),
                    Op::ModK => op_mod_k!(),
                    Op::IDivK => op_i_div_k!(),
                    Op::DivK => op_div_k!(),
                    Op::PowK => op_pow_k!(),
                    Op::BAndK => op_b_and_k!(),
                    Op::BOrK => op_b_or_k!(),
                    Op::BXorK => op_b_xor_k!(),
                    Op::ShrI => op_shr_i!(),
                    Op::ShlI => op_shl_i!(),
                    Op::Unm => op_unm!(),
                    Op::BNot => op_b_not!(),
                    Op::Not => op_not!(),
                    Op::Len => op_len!(),
                    Op::Jmp => op_jmp!(),
                    Op::Eq => op_eq!(),
                    Op::EqK => op_eq_k!(),
                    Op::Lt => order_arm!(<, false),
                    Op::Le => order_arm!(<=, true),
                    Op::EqI => op_eq_i!(),
                    Op::LtI => order_imm_arm!(<, false, false),
                    Op::LeI => order_imm_arm!(<=, false, true),
                    Op::GtI => order_imm_arm!(>, true, false),
                    Op::GeI => order_imm_arm!(>=, true, true),
                    Op::LtK => order_k_arm!(<, false),
                    Op::LeK => order_k_arm!(<=, true),
                    Op::Test => op_test!(),
                    Op::TestSet => op_test_set!(),
                    Op::ForLoop => op_for_loop!(),
                    Op::ForLoop55 => op_for_loop55!(),
                    Op::TForLoop => op_t_for_loop!(4, true),
                    Op::TForLoop53 => op_t_for_loop!(3, true),
                    Op::TForLoop55 => op_t_for_loop!(3, false),
                    Op::VargIdx => op_varg_idx!(),
                    Op::ErrNNil => op_err_n_nil!(),
                    Op::Call => op_call!(),
                    Op::Return0 => op_return0!(),
                    Op::Return1 => op_return1!(),
                    // they stay in this frame: run out of line, then go on
                    Op::LoadKx
                    | Op::NewTable
                    | Op::SetList
                    | Op::Pow
                    | Op::Concat
                    | Op::ForPrep
                    | Op::ForPrep55
                    | Op::TForPrep
                    | Op::TForPrep53
                    | Op::TForPrep55
                    | Op::Closure
                    | Op::Vararg
                    | Op::GetVarg
                    | Op::ShlK
                    | Op::ShrK
                    | Op::AddKK
                    | Op::SubKK
                    | Op::MulKK
                    | Op::ModKK
                    | Op::PowKK
                    | Op::DivKK
                    | Op::IDivKK
                    | Op::BAndKK
                    | Op::BOrKK
                    | Op::BXorKK
                    | Op::ShlKK
                    | Op::ShrKK
                    | Op::EqKK
                    | Op::LtKK
                    | Op::LeKK => {
                        save!();
                        self.run_frame_op(inst)?
                    }
                    // listed rather than `_`, so that the jump table covers every
                    // opcode without a range check
                    Op::Close
                    | Op::JmpClose
                    | Op::JmpCloseBack
                    | Op::Tbc
                    | Op::TailCall
                    | Op::Return
                    | Op::TForCall
                    | Op::TForCall53
                    | Op::TForCall55
                    | Op::ExtraArg => {
                        save!();
                        return Ok(FastExit::Slow(inst));
                    }
                }
                // a fast arm's slow path
                resume!()
            }
        }
    }
}
