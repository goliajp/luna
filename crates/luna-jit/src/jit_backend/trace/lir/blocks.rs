//! The blocks of a [`Lir`] being recorded: their predecessors, the
//! variables' values they inherit, their parameters, and the branches
//! between them.

use super::record::{v, val};
use super::*;
use cranelift_codegen::ir::{Block, BlockArg, Inst as CInst, Value};

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

    /// A new block, not entered and with no predecessors yet.
    pub(super) fn new_block(&mut self) -> Block {
        self.blocks.push(BlockData {
            first: NONE,
            last: NONE,
            inherit: (NONE, 0),
            ..BlockData::default()
        });
        Block::from_u32(self.blocks.len() as u32 - 1)
    }

    /// Makes `b` the current block, with the variables' values it
    /// inherits once it can no longer gain a predecessor.
    pub(super) fn enter_block(&mut self, b: Block) {
        let bi = b.as_u32();
        self.cur = bi;
        let blk = &mut self.blocks[bi as usize];
        let inherit = if blk.entered { (NONE, 0) } else { blk.inherit };
        blk.entered = true;
        let sealed = blk.sealed;
        self.var_cur.iter_mut().for_each(|x| *x = NONE);
        // a block entered before it is sealed may still gain a predecessor
        // (a loop head's back edge); its values are known only once it is
        // sealed with nothing emitted yet
        if inherit.0 != NONE {
            if sealed {
                self.restore(inherit);
                self.inherited.push((bi, inherit));
            } else {
                self.pending_inherit = Some((bi, inherit));
            }
        }
    }

    /// Marks `b` as having all its predecessors.
    pub(super) fn seal(&mut self, b: Block) {
        let bi = b.as_u32();
        let blk = &mut self.blocks[bi as usize];
        blk.sealed = true;
        if blk.first == NONE
            && bi == self.cur
            && let Some((pb, snap)) = self.pending_inherit.take()
            && pb == bi
        {
            self.restore(snap);
            self.inherited.push((bi, snap));
        }
    }

    pub(super) fn jump_to<'a>(
        &mut self,
        b: Block,
        args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> CInst {
        self.edge(b);
        let i = self.push(Op::Jump, Ty::I64, NONE, b.as_u32(), NONE, NONE);
        let at = self.args.len() as u32;
        let n = self.push_block_args(args);
        let inst = &mut self.insts[i as usize];
        (inst.args_at, inst.n_args) = (at, n);
        CInst::from_u32(i)
    }

    pub(super) fn branch<'a>(
        &mut self,
        c: Value,
        then_b: Block,
        then_args: impl IntoIterator<Item = &'a BlockArg>,
        else_b: Block,
        else_args: impl IntoIterator<Item = &'a BlockArg>,
    ) -> CInst {
        self.edge(then_b);
        self.edge(else_b);
        let at = self.args.len() as u32;
        let n_then = self.push_block_args(then_args);
        let n_else = self.push_block_args(else_args);
        let i = self.push(
            Op::Brif(n_then),
            Ty::I8,
            NONE,
            v(c),
            then_b.as_u32(),
            else_b.as_u32(),
        );
        let inst = &mut self.insts[i as usize];
        (inst.args_at, inst.n_args) = (at, n_then + n_else);
        CInst::from_u32(i)
    }

    /// See [`Op::Pick`].
    pub(super) fn pick_to(&mut self, loop_fn: Block, other: Block) {
        self.edge(loop_fn);
        self.edge(other);
        let (a, b) = (loop_fn.as_u32(), other.as_u32());
        self.push(Op::Pick, Ty::I64, NONE, a, b, NONE);
    }
}
