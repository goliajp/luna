//! The interpreter's fast loop: the opcodes that never call, allocate or
//! run a metamethod execute here on locals, in a function of their own so
//! that its register allocation does not depend on the rest of the loop.

use super::*;
use super::{for_loop::int_for_count, num_double as dbl};
use crate::runtime::value::tag;
use call_fast::Returned;
#[cfg(all(any(target_arch = "x86_64", target_arch = "x86"), not(miri)))]
use fast_arith::raw_tag_again;
use fast_arith::{
    arith_arm, arith_imm_arm, cold_path, is_num_tag, put_flt, put_int, raw_flt, raw_gc, raw_int,
    raw_num, raw_tag, raw_truthy, reg_a,
};
use index_fast::self_key;
use num_words::put_words;
#[cfg(all(target_arch = "aarch64", not(miri)))]
use num_words::{first, raw_tag_words3, raw_words3};
#[cfg(all(
    any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "x86"),
    not(miri)
))]
use num_words::{raw_word, tag_word};

#[cfg(not(luna_hot))]
mod arm_fns;
mod arm_helpers;
mod arms_arith;
mod arms_call;
mod arms_cmp;
mod arms_for_prep;
mod arms_load;
mod arms_loop;
mod arms_loop_a64;
mod dispatch;
mod flow;
#[cfg(not(luna_hot))]
mod fn_flow;
mod step;
mod table;
use arm_helpers::fast_arm_helper_macros;
use arms_arith::fast_arith_arms;
use arms_call::fast_call_arms;
use arms_cmp::fast_cmp_arms;
use arms_for_prep::fast_for_prep_arms;
use arms_load::fast_load_arms;
use arms_loop::fast_loop_arms;
use arms_loop_a64::fast_loop_a64_arms;
use dispatch::{fast_arm_families, fast_dispatch_macros};
use flow::fast_flow_macros;
#[cfg(not(luna_hot))]
use fn_flow::fast_fn_flow_macros;
use step::{fast_frame_macros, fast_step_macros};
use table::fast_arms;

mod frame;
pub(super) use frame::{Fast, FastExit};
#[cfg(not(luna_hot))]
use frame::{Flow, St};

impl Vm {
    /// The register window of a frame at `base`. It is valid while
    /// `self.stack` neither moves nor is written through a reference, and
    /// `push_frame` sized the stack to `base + max_stack`, so every
    /// register the frame's instructions name is inside it.
    #[cfg_attr(luna_hot, inline(always))]
    fn regs_at(&mut self, base: u32) -> *mut Value {
        self.stack.as_mut_ptr().wrapping_add(base as usize)
    }

    /// Run instructions from `inst` (at `npc - 1`) until one needs the loop
    /// head. The frame's pc is `npc` on entry and is kept current. `WATCH`
    /// is false when `fx.stay` holds and `fx.heads` is empty: that loop then
    /// tests nothing per instruction. Without `WATCH`, `TRACE` is
    /// `fx.trace_on`, fixed for the loop so that it costs nothing. `DBL`
    /// is true for 5.1/5.2, whose integers stand for doubles (see
    /// [`super::num_double`]); `PRE53` for 5.1-5.3, whose numeric `for`
    /// steps before it tests and compares with the limit.
    #[inline(never)]
    // the step and flow macros serve the arms' functions too
    #[allow(unused_macros)]
    pub(super) fn run_fast<
        const WATCH: bool,
        const TRACE: bool,
        const DBL: bool,
        const PRE53: bool,
    >(
        &mut self,
        fx: Fast,
        mut inst: Inst,
        mut npc: u32,
    ) -> Result<FastExit, LuaError> {
        let Fast {
            mut fr,
            trace_on: trace_rt,
            entry_depth,
            stay,
            heads,
        } = fx;
        let trace_on = if WATCH { trace_rt } else { TRACE };
        #[cfg(luna_hot)]
        let pre53 = PRE53;
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
        fast_frame_macros!($, fr);
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
            fast_flow_macros!($, self, fr, regs, npc, inst, code, kptr, trace_on, entry_depth, stay, heads, 'frames);
            fast_dispatch_macros!($, self, fr, regs, npc, inst, code, kptr, trace_on, entry_depth, stay, heads, 'frames);
            loop {
                // the instruction now running
                #[cfg(luna_hot)]
                let pc = npc - 1;
                #[cfg(luna_hot)]
                fast_arm_families!($, self, fr, regs, npc, inst, pc, code, kptr, trace_on, pre53, entry_depth, 'frames);
                fast_arms!(dispatch_arms, inst);
            }
        }
    }
}
