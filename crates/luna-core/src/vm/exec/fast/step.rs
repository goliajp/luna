//! Register access and how the fast loop moves on: fetch the next
//! instruction, take a comparison's jump, store the pc, find the running
//! frame. The generator defines the macros inside `run_fast`; it takes the
//! loop's locals as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_step_macros {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $code:ident,
        $trace_on:ident, $stay:ident, $heads:ident
    ) => {
        macro_rules! reg {
            ($d i:expr) => {
                // SAFETY: registers are below `max_stack` (see `Vm::r`)
                unsafe { *$regs.add(($d i) as usize) }
            };
        }
        macro_rules! set_reg {
            ($d i:expr, $d v:expr) => {
                // SAFETY: as for `reg!`
                unsafe { *$regs.add(($d i) as usize) = $d v }
            };
        }
        // store `npc` as the running frame's pc
        macro_rules! store_pc {
            () => {
                // SAFETY: `fr` points at the running frame, the top of
                // `frames`, which no fast arm pushes or pops without taking
                // `fr` again
                unsafe { (*$fr).pc = $npc }
            };
        }
        // the instruction at `pc` of the running function
        macro_rules! fetch {
            ($d pc:expr) => {
                // `code` is the running proto's code; the compiler
                // and the bytecode verifier keep every pc an instruction
                // reaches (the next one, a jump target, the `Jmp` after a
                // test) inside it, and every function ends in a return
                {
                    let pc: u32 = $d pc;
                    // SAFETY: see above
                    unsafe { *$code.add(pc as usize) }
                }
            };
        }
        // Lua truth of register `i`
        macro_rules! reg_truthy {
            ($d i:expr) => {{
                let i: u32 = $d i;
                // SAFETY: as for `reg!`
                unsafe { raw_truthy($regs.add(i as usize)) }
            }};
        }
        macro_rules! next {
            () => {{
                // nothing in a fast arm sets `trap`, so `stay` holds for the
                // whole loop
                if WATCH && !$stay {
                    store_pc!();
                    return Ok(FastExit::Reload);
                }
                $inst = fetch!($npc);
                $npc += 1;
                if WATCH {
                    store_pc!();
                }
                continue;
            }};
        }
        // after a jump: with a trace this function could enter, stop where
        // one starts, for the dispatcher. Trace heads are loop heads, and a
        // loop entered by falling into it is caught at its first back-edge;
        // checking every instruction cost a call-heavy loop 5%.
        macro_rules! next_jumped {
            () => {{
                if WATCH && $heads.contains(&$npc) {
                    store_pc!();
                    return Ok(FastExit::Reload);
                }
                next!()
            }};
        }
        // The end of a comparison or a test: the `Jmp` after it runs when
        // the outcome equals `k`, and is skipped otherwise. Without anything
        // to watch the jump is taken here (PUC `donextjump`), which saves its
        // dispatch and makes the two outcomes different code, so that the
        // compiler branches on the outcome instead of computing the next pc
        // from it. A back-edge is left to the `Jmp` arm when the trace JIT
        // counts back-edges.
        macro_rules! cond_jump {
            ($d taken:expr) => {{
                if !$d taken {
                    $npc += 1;
                } else if !WATCH {
                    // the compiler and the bytecode verifier put a jump
                    // after every comparison and test; one that closes runs
                    // as an instruction of its own
                    let j = fetch!($npc);
                    let off = j.sj();
                    if j.op() == Op::Jmp && !($trace_on && off < 0) {
                        $npc = ($npc as i64 + 1 + off as i64) as u32;
                    }
                }
                next!()
            }};
        }
        // Without anything to watch the frame's pc is stored only before
        // whatever can read it: a call, a metamethod, an error, the loop
        // head (PUC `savepc`). Every slow path below starts with this.
        macro_rules! save {
            () => {
                if !WATCH {
                    store_pc!();
                }
            };
        }
        // the top frame, known to be a Lua frame while `trap` is clear
        macro_rules! top_lua {
            () => {{
                debug_assert!(!$vm.trap);
                // SAFETY: the running thread has a frame, and with `trap`
                // clear the top one is not a continuation (see
                // `frames_pop_sync`)
                unsafe {
                    match $vm.frames.last_mut().unwrap_unchecked() {
                        CallFrame::Lua(f) => f,
                        CallFrame::Cont(_) => std::hint::unreachable_unchecked(),
                    }
                }
            }};
        }
    };
}
pub(super) use fast_step_macros;
