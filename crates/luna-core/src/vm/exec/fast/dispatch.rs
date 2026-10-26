//! The fast loop's dispatch, built from the table (`fast_arms`).
//! Optimised (`luna_hot`), each arm's macro is expanded where its opcode
//! dispatches, with a dispatch of its own after it. Unoptimised, one call
//! site serves every arm's function (see `arm_fns`), so that the loop's
//! frame holds one set of locals rather than every arm's.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_dispatch_macros {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $code:ident,
        $kptr:ident, $trace_on:ident, $entry_depth:ident, $stay:ident, $heads:ident,
        $frames:lifetime
    ) => {
        #[cfg(luna_hot)]
        macro_rules! dispatch_arms {
            (
                ($d i:ident) [$d($d op:ident $d name:ident $d body:expr;)*]
                [$d($d fo:ident)*] [$d($d sl:ident)*]
            ) => {{
                match $inst.op() {
                    $d(Op::$d op => $d body,)*
                    $d(Op::$d fo)|* => {
                        save!();
                        $vm.run_frame_op($inst)?
                    }
                    $d(Op::$d sl)|* => {
                        save!();
                        return Ok(FastExit::Slow($inst));
                    }
                }
                // an arm's slow path, or the frame op
                resume!()
            }};
        }
        #[cfg(not(luna_hot))]
        macro_rules! dispatch_arms {
            (
                ($d i:ident) [$d($d op:ident $d name:ident $d body:expr;)*]
                [$d($d fo:ident)*] [$d($d sl:ident)*]
            ) => {{
                let arm: frame::ArmFn = match $inst.op() {
                    $d(Op::$d op => Vm::$d name::<WATCH, TRACE, DBL, PRE53>,)*
                    $d(Op::$d fo)|* => {
                        save!();
                        $vm.run_frame_op($inst)?;
                        resume!()
                    }
                    $d(Op::$d sl)|* => {
                        save!();
                        return Ok(FastExit::Slow($inst));
                    }
                };
                let mut st = St {
                    fr: $fr, regs: $regs, npc: $npc, inst: $inst, code: $code, kptr: $kptr,
                    trace_on: $trace_on, entry_depth: $entry_depth, stay: $stay, heads: $heads,
                };
                let flow = arm($vm, &mut st)?;
                ($fr, $regs, $npc, $inst, $code, $kptr) =
                    (st.fr, st.regs, st.npc, st.inst, st.code, st.kptr);
                match flow {
                    Flow::Next => next!(),
                    Flow::NextJumped => next_jumped!(),
                    Flow::Resume => resume!(),
                    Flow::ResumeSame => resume_same!(),
                    Flow::Reenter => reenter!(),
                    Flow::Frames => continue $frames,
                    Flow::Returned(done) => returned!(done),
                    Flow::Exit(e) => return Ok(e),
                }
            }};
        }
    };
}
pub(super) use fast_dispatch_macros;

/// Every arm's macro (and the helpers they share), over the loop's locals.
// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_arm_families {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        fast_arm_helper_macros!($d, $vm, $fr, $regs, $npc, $inst, $code, $kptr, $trace_on, $entry_depth, $frames);
        fast_load_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_arith_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_cmp_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_for_prep_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_loop_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_loop_a64_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
        fast_call_arms!($d, $vm, $fr, $regs, $npc, $inst, $pc, $code, $kptr, $trace_on, $pre53, $entry_depth, $frames);
    };
}
pub(super) use fast_arm_families;
