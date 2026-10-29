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
/// reached from code that is not.
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
