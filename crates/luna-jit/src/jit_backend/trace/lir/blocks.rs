//! The blocks of a [`Lir`] being recorded: their predecessors, the
//! variables' values they inherit, and their parameters.

use super::record::val;
use super::*;
use cranelift_codegen::ir::Block;

impl Lir {
    /// A branch from the current block to `b`. A block not entered yet
    /// keeps the variables' values every branch into it agrees on.
    pub(super) fn edge(&mut self, b: Block) {
        let bi = b.as_u32() as usize;
        let blk = &mut self.blocks[bi];
        blk.preds += 1;
        if blk.entered {
            // a back edge: what the block assumed on entry may not hold
            if self
                .pending_inherit
                .is_some_and(|(pb, _)| pb as usize == bi)
            {
                self.pending_inherit = None;
            }
            return;
        }
        let (at, n) = blk.inherit;
        if blk.preds == 1 {
            let at = self.snaps.len() as u32;
            self.snaps.extend_from_slice(&self.var_cur);
            self.blocks[bi].inherit = (at, self.var_cur.len() as u32);
        } else if at != NONE {
            for k in 0..n as usize {
                let s = &mut self.snaps[at as usize + k];
                if *s != self.var_cur[k] {
                    *s = NONE;
                }
            }
        }
    }

    /// Appends `d` to block `b`'s parameters, moving them to the end of the
    /// pool first when another block's were appended since.
    pub(super) fn add_block_param(&mut self, b: u32, d: u32) {
        let (at, n) = {
            let blk = &self.blocks[b as usize];
            (blk.params_at as usize, blk.n_params as usize)
        };
        if n == 0 || at + n != self.bparams.len() {
            let new_at = self.bparams.len();
            self.bparams.extend_from_within(at..at + n);
            self.bparam_vals.extend_from_within(at..at + n);
            self.blocks[b as usize].params_at = new_at as u32;
        }
        self.bparams.push(d);
        self.bparam_vals.push(val(d));
        self.blocks[b as usize].n_params += 1;
    }

    pub(super) fn restore(&mut self, (at, n): (u32, u32)) {
        self.var_cur[..n as usize].copy_from_slice(&self.snaps[at as usize..(at + n) as usize]);
    }
}
