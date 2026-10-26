//! Where the fast loop goes after a call, a return or a slow path. The
//! generator defines the macros inside `run_fast`'s `'frames` loop; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_flow_macros {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $code:ident,
        $kptr:ident, $trace_on:ident, $entry_depth:ident, $stay:ident, $heads:ident,
        $frames:lifetime
    ) => {
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
        // the frame `fr` now names, from its pc
        macro_rules! frames {
            () => {
                continue $frames
            };
        }
        // back to the loop head
        macro_rules! exit {
            ($d e:expr) => {
                return Ok($d e)
            };
        }
        // after a call or return: take on whatever frame is now on top
        macro_rules! reenter {
            () => {{
                if $vm.trap && !$vm.settle_frames($entry_depth)? {
                    return Ok(FastExit::Reload);
                }
                $fr = top_lua!();
                continue $frames;
            }};
        }
        // after a slow path: the same frame, whose pc it may have moved (a
        // comparison's skip) and whose stack it may have written, unless
        // it called a metamethod
        macro_rules! resume {
            () => {{
                if $vm.trap {
                    reenter!()
                }
                let f = top_lua!();
                $npc = f.pc;
                let base = f.base;
                $fr = f;
                $regs = $vm.regs_at(base);
                next!()
            }};
        }
        // after a slow path that moves neither the pc nor the stack and
        // pushes a frame only for a metamethod, which sets `trap`: a
        // table read or write, an arithmetic or unary fallback
        macro_rules! resume_same {
            () => {{
                if $vm.trap {
                    reenter!()
                }
                next!()
            }};
        }
        // after `return_fast`: a Lua caller is taken up from its frame
        macro_rules! returned {
            ($d done:expr) => {{
                match $d done {
                    Returned::ToLua(f) => {
                        $fr = f;
                        continue $frames;
                    }
                    Returned::No => {
                        save!();
                        return Ok(FastExit::Slow($inst));
                    }
                    _ => reenter!(),
                }
            }};
        }
    };
}
pub(super) use fast_flow_macros;
