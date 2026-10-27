//! The blocks PUC's undumper asks its allocator for whose sizes the
//! chunk gives: an array before its elements are read, a long string
//! before its bytes. A size the bytes left cannot back is the allocator's
//! to refuse, and PUC then ends the load with the memory error; an array
//! whose bytes do not fit a `size_t` it refuses itself (`luaM_toobig`).

use std::alloc::Layout;

use super::*;
use crate::runtime::mem::BlockKind;
use crate::vm::dump::UEv;
use undump::parser_array;

impl Vm {
    /// Where one of the blocks of a refused binary chunk is refused, in
    /// the order PUC asks for them: the event of the array, or `evs.len()`
    /// for the cut long string of `cut_long` bytes, and whether it is too
    /// big for a `size_t`; `None` when all are given. `evs` is cut to what
    /// PUC made before.
    pub(super) fn refused_block(
        &mut self,
        evs: &mut Vec<UEv>,
        cut_long: Option<usize>,
    ) -> Option<(usize, bool)> {
        let v = self.version;
        let arrays = evs.iter().enumerate().filter_map(|(i, ev)| match *ev {
            UEv::Vec(a, n) => {
                let elem = self.heap.parser_bytes(parser_array(a), 1);
                // the `int` count as a `size_t` (5.3 takes a negative one)
                let n = n as i32 as isize as usize;
                // `luaM_reallocv` / `luaM_testsize`: from 5.3 on only where
                // an `int` is as wide as a `size_t`
                let tested = v <= LuaVersion::Lua52 || usize::BITS <= 32;
                if tested && n.wrapping_add(1) > usize::MAX / elem {
                    return Some((i, None));
                }
                Some((i, Some(n.wrapping_mul(elem))))
            }
            _ => None,
        });
        let long = cut_long.map(|len| {
            let size = len
                .checked_add(1 << 8)
                .map(|_| self.heap.puc_bytes(PucObj::Str { len, long: true }));
            (evs.len(), Some(size.unwrap_or(usize::MAX)))
        });
        let blocks: Vec<(usize, Option<usize>)> = arrays.chain(long).collect();
        let refused = blocks
            .into_iter()
            .find(|&(_, size)| size.is_none_or(|s| !self.block_given(s)))
            .map(|(at, size)| (at, size.is_none()));
        evs.truncate(refused.map_or(evs.len(), |r| r.0));
        refused
    }

    /// The error of a load PUC's undumper ends at a refused block: the
    /// memory error, or (`too_big`) `luaM_toobig`'s runtime error, which
    /// the message handler running then gets.
    pub(super) fn refused_block_error(&mut self, too_big: bool) -> SyntaxError {
        if !too_big {
            return SyntaxError::memory();
        }
        let e = self.raise_msg(None, b"memory allocation error: block too big", &[]);
        let v = self.load_error_handled(e.0);
        self.load_failed_by(LuaError(v))
    }

    /// Whether the allocator gives a block of `size` bytes, asked as PUC's
    /// `luaM_malloc_` asks it: again after an emergency collection.
    fn block_given(&mut self, size: usize) -> bool {
        let given = |vm: &Vm| {
            let Ok(l) = Layout::from_size_align(size.max(1), 16) else {
                return false;
            };
            let ctx = vm.heap.mem().ctx();
            let Some(p) = ctx.alloc(l, BlockKind::Other) else {
                return false;
            };
            // SAFETY: `p` is the block of `l` just allocated from `ctx`, used by nothing
            unsafe { ctx.free(p, l) };
            true
        };
        given(self) || (self.emergency_collect() && given(self))
    }
}
