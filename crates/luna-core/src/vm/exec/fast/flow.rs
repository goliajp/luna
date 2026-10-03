//! Where the fast loop goes after a call, a return or a slow path. The
//! generator defines the macros inside `run_fast`'s `'frames` loop; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_flow_macros {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $code:ident,
        $kptr:ident, $trace_on:ident, $entry_depth:ident, $frames:lifetime
    ) => {
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
