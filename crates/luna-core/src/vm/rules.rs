//! Library code built for one of the C API's two rules of where it checks
//! for a collection, a `const PRE: bool` parameter: `true` for 5.1 and
//! 5.2, whose API checks before it allocates, `false` from 5.3 on, which
//! check after it pushed what it made. The tables of PUC's functions hold
//! both builds; a VM opens the one of its dialect, so the checks read no
//! rule as they run.

/// `[f::<false>, f::<true>]`: a library function built for each rule, as
/// [`crate::vm::opening::Ctor::R`] holds it.
macro_rules! by_rules {
    ($f:ident) => {
        [$f::<false>, $f::<true>]
    };
}

/// `e` with `P` the rule of the VM `vm`, where code built for one rule is
/// reached from code that is not. Those are the places no table of PUC's
/// functions leads to, which so carry no rule of their own: the VM's own
/// entries (`tostring`, the names it pushes, its C API calls, the
/// script's `arg`), and helpers several library functions share (an I/O
/// result, a buffer's string, an error message, a traceback, an option's
/// check), whose callers are built both ways or neither. Giving each of
/// them the parameter would build two copies of every path through them.
macro_rules! with_rules {
    ($vm:expr, $p:ident => $e:expr) => {
        if $vm.heap.rules().api_checks_before() {
            const $p: bool = true;
            $e
        } else {
            const $p: bool = false;
            $e
        }
    };
}
