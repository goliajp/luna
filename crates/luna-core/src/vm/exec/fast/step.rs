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
        macro_rules! next {
            () => {{
                // nothing in a fast arm sets `trap`, so `stay` holds for the
                // whole loop; with a trace this function could enter, the
                // arms stop at the pcs where one starts, for the dispatcher
                if WATCH && (!$stay || $heads.contains(&$npc)) {
                    // SAFETY: `fr` is the running frame, which no fast arm
                    // moves
                    unsafe { (*$fr).pc = $npc };
                    return Ok(FastExit::Reload);
                }
                // SAFETY: as for the fetch at the loop head
                $inst = unsafe { *$code.add($npc as usize) };
                $npc += 1;
                if WATCH {
                    // SAFETY: see above
                    unsafe { (*$fr).pc = $npc };
                }
                continue;
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
                    // SAFETY: the compiler and the bytecode verifier put a
                    // `Jmp` after every comparison and test
                    let j = unsafe { *$code.add($npc as usize) };
                    debug_assert!(j.op() == Op::Jmp);
                    let off = j.sj();
                    if !($trace_on && off < 0) {
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
                    // SAFETY: `fr` is the running frame
                    unsafe { (*$fr).pc = $npc };
                }
            };
        }
        // the top frame, known to be a Lua frame while `trap` is clear
        macro_rules! top_lua {
            () => {{
                debug_assert!(!$vm.trap);
                // SAFETY: the running thread has a frame, and with `trap`
                // clear it is not a continuation (see `frames_pop_sync`)
                match unsafe { $vm.frames.last_mut().unwrap_unchecked() } {
                    CallFrame::Lua(f) => f,
                    // SAFETY: see above
                    CallFrame::Cont(_) => unsafe { std::hint::unreachable_unchecked() },
                }
            }};
        }
    };
}
pub(super) use fast_step_macros;
