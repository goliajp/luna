//! Where the build does not optimise (no `luna_hot`), each arm of the fast
//! loop is a function of its own, which `run_fast` calls from one place:
//! its frame then holds none of the arms' locals. Each function
//! instantiates the arm macros over its own copies of the loop's locals
//! and hands back where the loop goes next ([`Flow`]), its locals written
//! back to the [`St`] it was given. Optimised, the arms are expanded in
//! `run_fast` itself and these functions are not built.

use super::frame::{Flow, St};
use super::*;

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! arm_fns {
    (($d:tt [$fr:ident $regs:ident $npc:ident $inst:ident $code:ident $kptr:ident]) [$($op:ident $name:ident $body:expr;)*] [$($_frame_op:ident)*] [$($_slow:ident)*]) => {
        impl Vm {
            $(
                #[allow(unused_mut, unused_variables, unused_macros, unused_assignments, unreachable_code)]
                #[allow(clippy::needless_return, clippy::diverging_sub_expression)]
                pub(super) fn $name<
                    const WATCH: bool,
                    const TRACE: bool,
                    const DBL: bool,
                    const PRE53: bool,
                >(
                    &mut self,
                    st: &mut St,
                ) -> Result<Flow, LuaError> {
                    let St { mut $fr, mut $regs, mut $npc, mut $inst, mut $code, mut $kptr, trace_on, entry_depth, stay, heads } = *st;
                    let pre53 = PRE53;
                    // the instruction now running
                    let pc = $npc - 1;
                    macro_rules! leave {
                        ($d flow:expr) => {{
                            *st = St { $fr, $regs, $npc, $inst, $code, $kptr, trace_on, entry_depth, stay, heads };
                            return Ok($d flow);
                        }};
                    }
                    fast_frame_macros!($d, $fr);
                    fast_step_macros!($d, self, $fr, $regs, $npc, $inst, $code, trace_on, stay, heads);
                    fast_fn_flow_macros!($d);
                    fast_arm_families!($d, self, $fr, $regs, $npc, $inst, pc, $code, $kptr, trace_on, pre53, entry_depth, 'frames);
                    $body;
                    // an arm that ends without going on: its slow path ran
                    #[allow(unreachable_code)]
                    {
                        leave!(Flow::Resume)
                    }
                }
            )*
        }
    };
}

fast_arms!(arm_fns, $ [fr regs npc inst code kptr]);
