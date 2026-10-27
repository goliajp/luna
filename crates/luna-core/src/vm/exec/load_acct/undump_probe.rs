//! The blocks PUC's undumper asks its allocator for whose sizes the
//! chunk gives: an array before its elements are read, a long string
//! before its bytes. A size the bytes left cannot back is the allocator's
//! to refuse, and PUC then ends the load with the memory error.

use std::alloc::Layout;

use super::*;
use crate::runtime::mem::BlockKind;
use crate::vm::dump::UEv;
use undump::parser_array;

impl Vm {
    /// Where the allocator refuses one of the blocks of a refused binary
    /// chunk, in the order PUC asks for them: the event of the array, or
    /// `evs.len()` for the cut long string of `cut_long` bytes; `None`
    /// when it gives them all. `evs` is cut to what PUC made before.
    pub(super) fn refused_block(
        &mut self,
        evs: &mut Vec<UEv>,
        cut_long: Option<usize>,
    ) -> Option<usize> {
        let arrays = evs.iter().enumerate().filter_map(|(i, ev)| match *ev {
            UEv::Vec(a, n) => Some((i, self.heap.parser_bytes(parser_array(a), n as usize))),
            _ => None,
        });
        let long = cut_long.map(|len| {
            (
                evs.len(),
                self.heap.puc_bytes(PucObj::Str { len, long: true }),
            )
        });
        let blocks: Vec<(usize, usize)> = arrays.chain(long).collect();
        let refused = blocks
            .into_iter()
            .find(|&(_, size)| !self.block_given(size))
            .map(|(at, _)| at);
        evs.truncate(refused.unwrap_or(evs.len()));
        refused
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
