//! The dialects' rules the heap follows where it does what PUC's collector
//! does: which objects PUC has, where they sit on its list, and what its
//! steps count. The heap asks these by name and keeps no rule of its own.

use super::*;
use crate::runtime::dialect::Dialect;

/// Whether PUC's C function with `nup` upvalues is an object: from 5.2 on
/// one without upvalues is a light function.
#[inline]
pub(crate) fn c_function_is_object(d: Dialect, nup: usize) -> bool {
    nup > 0 || d == Dialect::L51
}

impl Heap {
    /// Whether PUC's C function with `nup` upvalues is an object.
    #[inline]
    pub(in crate::runtime) fn puc_c_function_is_object(&self, nup: usize) -> bool {
        c_function_is_object(self.puc.dialect, nup)
    }

    /// The `CallInfo`s PUC counts for thread `c`: 5.1's array, the later
    /// dialects' list.
    #[inline]
    pub(in crate::runtime) fn puc_thread_ci(&self, c: &crate::runtime::Coro) -> u32 {
        if self.puc.dialect == Dialect::L51 {
            c.frame_size
        } else {
            c.ci_alloc
        }
    }

    /// The stack a new thread starts with (`stack_init`'s
    /// `BASIC_STACK_SIZE`, 5.1 with its extra slots).
    #[inline]
    pub(in crate::runtime) fn puc_basic_stack(&self) -> u32 {
        if self.puc.dialect == Dialect::L51 {
            45
        } else {
            40
        }
    }

    /// A new thread's `CallInfo`s: 5.1's array (`frame_size`), the later
    /// dialects' list (`ci_alloc`); `u32::MAX` for the one the dialect has
    /// not.
    #[inline]
    pub(in crate::runtime) fn puc_new_thread_ci(&self) -> (u32, u32) {
        if self.puc.dialect == Dialect::L51 {
            (super::super::BASIC_CI_51, u32::MAX)
        } else {
            (u32::MAX, 0)
        }
    }

    /// Up to 5.3 a string table that cannot grow is an error when it is
    /// full (`luaS_resize` raises); later ones go on with longer chains.
    #[inline]
    pub(in crate::runtime) fn puc_strt_grows_strictly(&self) -> bool {
        self.puc.dialect <= Dialect::L53
    }

    /// 5.1: the main thread is on PUC's list, its oldest object, and each
    /// new userdata goes after it.
    #[inline]
    pub(in crate::runtime) fn puc_udata_after_main_thread(&self) -> bool {
        self.puc.dialect == Dialect::L51
    }

    /// 5.1 stops the collector with an unreachable threshold; the later
    /// dialects still step while it is stopped, to put the debt back.
    #[inline]
    pub(in crate::runtime) fn puc_stop_unreachable(&self) -> bool {
        self.puc.dialect == Dialect::L51
    }

    /// 5.1's full collection sweeps the string table before the list.
    #[inline]
    pub(in crate::runtime) fn puc_full_sweeps_strings_first(&self) -> bool {
        self.puc.dialect == Dialect::L51
    }

    /// 5.1 counts the work of marking what objects to be finalized keep
    /// alive (`propagateall`).
    #[inline]
    pub(in crate::runtime) fn puc_counts_finalizable_work(&self) -> bool {
        self.puc.dialect == Dialect::L51
    }

    /// 5.2's fixed strings are white like any other: a cycle marks them
    /// where it reaches them, and counts them.
    #[inline]
    pub(in crate::runtime) fn puc_fixed_strings_marked(&self) -> bool {
        self.puc.dialect == Dialect::L52
    }

    /// 5.2 and 5.3 count the strings and userdata a traversal marks: the
    /// header of each.
    #[inline]
    pub(in crate::runtime) fn puc_leaf_hdr(&self) -> usize {
        match self.puc.dialect {
            Dialect::L52 | Dialect::L53 => self.puc_size(PucObj::StrHeader),
            _ => 0,
        }
    }

    /// 5.2 counts a closed upvalue a traversal marks.
    #[inline]
    pub(in crate::runtime) fn puc_leaf_upval(&self) -> usize {
        match self.puc.dialect {
            Dialect::L52 => self.puc_size(PucObj::Upval),
            _ => 0,
        }
    }

    /// 5.3 shrinks a thread as it traverses it, once a cycle.
    #[inline]
    pub(in crate::runtime) fn puc_shrinks_traversed(&mut self, h: *mut GcHeader) -> bool {
        self.puc.dialect == Dialect::L53 && self.puc_shrinks_now(h)
    }

    /// The atomic step counts the work of what the barriers sent gray again
    /// (`grayagain`); 5.2 and 5.3 do not.
    #[inline]
    pub(in crate::runtime) fn puc_atomic_counts_again(&self) -> bool {
        !matches!(self.puc.dialect, Dialect::L52 | Dialect::L53)
    }

    /// What the atomic step's `entersweep` sweeps: 5.1 nothing, 5.3 one
    /// object, the others to the first object kept (`None`: nothing).
    #[inline]
    pub(in crate::runtime) fn puc_enter_sweep(&self) -> Option<(usize, bool)> {
        match self.puc.dialect {
            Dialect::L51 => None,
            Dialect::L53 => Some((1, false)),
            _ => Some((usize::MAX, true)),
        }
    }
}
