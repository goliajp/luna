//! The dialects' rules the heap follows where it does what PUC's collector
//! does: which objects PUC has, where they sit on its list, and what its
//! steps count. Decided once per dialect; the heap reads them from
//! [`Heap::puc_rules`] and keeps no rule of its own.

use super::*;
use crate::runtime::dialect::Dialect;
use crate::runtime::puc::size::PucSizes;

/// The rules of one dialect, as the heap reads them.
#[derive(Clone, Copy, Debug)]
pub(in crate::runtime) struct HeapRules {
    dialect: Dialect,
    /// from 5.2 on a C function without upvalues is a light function, not
    /// an object
    light_c_functions: bool,
    /// 5.1 counts a thread's `CallInfo` array, the later dialects its list
    ci_array: bool,
    /// a new thread's `CallInfo`s: 5.1's array (`frame_size`), the later
    /// dialects' list (`ci_alloc`); `u32::MAX` for the one the dialect has
    /// not
    pub(in crate::runtime) new_frame_size: u32,
    pub(in crate::runtime) new_ci_alloc: u32,
    /// the stack a new thread starts with (`stack_init`'s
    /// `BASIC_STACK_SIZE`, 5.1 with its extra slots)
    pub(in crate::runtime) basic_stack: u32,
    /// 5.1: the main thread is on PUC's list, its oldest object, and each
    /// new userdata goes after it
    pub(in crate::runtime) udata_after_main_thread: bool,
    /// 5.1 stops the collector with an unreachable threshold; the later
    /// dialects still step while it is stopped, to put the debt back
    pub(in crate::runtime) stop_unreachable: bool,
    /// 5.1's full collection sweeps the string table before the list
    pub(in crate::runtime) full_sweeps_strings_first: bool,
    /// 5.1 counts the work of marking what objects to be finalized keep
    /// alive (`propagateall`)
    pub(in crate::runtime) counts_finalizable_work: bool,
    /// 5.2's fixed strings are white like any other: a cycle marks them
    /// where it reaches them, and counts them
    pub(in crate::runtime) fixed_strings_marked: bool,
    /// what 5.2 and 5.3 count for each string and userdata a traversal
    /// marks: its header
    pub(in crate::runtime) leaf_hdr: usize,
    /// what 5.2 counts for each closed upvalue a traversal marks
    pub(in crate::runtime) leaf_upval: usize,
    /// 5.3 shrinks a thread as it traverses it, once a cycle
    pub(in crate::runtime) shrink_on_traverse: bool,
    /// the atomic step counts the work of what the barriers sent gray
    /// again (`grayagain`); 5.2 and 5.3 do not
    pub(in crate::runtime) atomic_counts_again: bool,
    /// what the atomic step's `entersweep` sweeps: 5.1 nothing, 5.3 one
    /// object, the others to the first object kept
    pub(in crate::runtime) enter_sweep: Option<(usize, bool)>,
    /// up to 5.3 a string table that cannot grow is an error when it is
    /// full (`luaS_resize` raises); later ones go on with longer chains
    pub(in crate::runtime) strt_grows_strictly: bool,
    /// PUC keeps the strings (5.1: every one, 5.2: the short ones) in its
    /// string table, swept apart from the list
    pub(in crate::runtime) sweeps_string_table: bool,
}

impl HeapRules {
    pub(in crate::runtime) fn of(d: Dialect, sizes: &PucSizes) -> HeapRules {
        let l51 = d == Dialect::L51;
        HeapRules {
            dialect: d,
            light_c_functions: !l51,
            ci_array: l51,
            new_frame_size: if l51 {
                super::super::BASIC_CI_51
            } else {
                u32::MAX
            },
            new_ci_alloc: if l51 { u32::MAX } else { 0 },
            basic_stack: if l51 { 45 } else { 40 },
            udata_after_main_thread: l51,
            stop_unreachable: l51,
            full_sweeps_strings_first: l51,
            counts_finalizable_work: l51,
            fixed_strings_marked: d == Dialect::L52,
            leaf_hdr: match d {
                Dialect::L52 | Dialect::L53 => PucObj::StrHeader.bytes(d, sizes),
                _ => 0,
            },
            leaf_upval: match d {
                Dialect::L52 => PucObj::Upval.bytes(d, sizes),
                _ => 0,
            },
            shrink_on_traverse: d == Dialect::L53,
            atomic_counts_again: !matches!(d, Dialect::L52 | Dialect::L53),
            enter_sweep: match d {
                Dialect::L51 => None,
                Dialect::L53 => Some((1, false)),
                _ => Some((usize::MAX, true)),
            },
            strt_grows_strictly: d <= Dialect::L53,
            sweeps_string_table: matches!(d, Dialect::L51 | Dialect::L52),
        }
    }

    /// The dialect these are the rules of.
    #[inline(always)]
    pub(in crate::runtime) fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Whether PUC's C function with `nup` upvalues is an object.
    #[inline(always)]
    pub(in crate::runtime) fn c_function_is_object(&self, nup: usize) -> bool {
        nup > 0 || !self.light_c_functions
    }

    /// Whether PUC has native `n` as an object.
    #[inline(always)]
    pub(in crate::runtime) fn native_is_object(&self, n: &crate::runtime::NativeClosure) -> bool {
        self.c_function_is_object(super::super::native_puc_upvals(n))
    }

    /// The `CallInfo`s PUC counts for thread `c`.
    #[inline]
    pub(in crate::runtime) fn thread_ci(&self, c: &crate::runtime::Coro) -> u32 {
        if self.ci_array {
            c.frame_size
        } else {
            c.ci_alloc
        }
    }
}

impl Heap {
    /// The rules of the model's dialect.
    #[inline(always)]
    pub(in crate::runtime) fn puc_rules(&self) -> &HeapRules {
        &self.puc.rules
    }
}
