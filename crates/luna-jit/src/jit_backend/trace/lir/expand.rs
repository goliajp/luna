//! The calls of a trace's calling exits, made into a copy of its
//! instructions once one of its side cells holds a side trace: the
//! lowering leaves each such exit a return and the gate a jump on (see
//! `lower::relinks`), so that a trace whose cells stay empty compiles and
//! is taken up by other Vms without any of it.

use super::record::val;
use super::*;
use crate::jit_backend::trace::RelocKind;
use cranelift_codegen::ir::{Block, Value};

/// `body`, the instructions of a trace with calling exits as lowered, with
/// their calls made, its relocations' values `relocs` and its first side
/// cell at `cells`: what every backend compiles once a cell is filled.
pub(crate) fn expand(body: &Lir, relocs: &[(RelocKind, i64)], cells: i64) -> Lir {
    let x = body.relinks.clone().expect("a trace with calling exits");
    let mut l = body.detach();
    l.relinks = None;
    l.relocs.clear();
    l.relocs.extend_from_slice(relocs);
    l.relocs.push((RelocKind::SideCells, cells));
    l.konst = vec![None; l.value_ty.len()];
    l.var_cur = vec![NONE; l.var_ty.len()];
    l.bparam_vals = l.bparams.iter().map(|&p| val(p)).collect();
    // nothing is lowered into the blocks again: a branch into one of them
    // assumes nothing of the variables
    for b in &mut l.blocks {
        (b.entered, b.sealed, b.inherit) = (true, true, (NONE, 0));
    }
    super::super::lower::expand_calls(&mut l, &x);
    l.cur = NONE;
    l
}

impl Lir {
    /// Makes `b` current again, its last instruction taken off its list,
    /// which is returned; the variables' values unknown.
    pub(in crate::jit_backend::trace) fn reopen(&mut self, b: Block) -> u32 {
        let bi = b.as_u32() as usize;
        let (first, last) = (self.blocks[bi].first, self.blocks[bi].last);
        let mut prev = NONE;
        let mut i = first;
        while i != last {
            prev = i;
            i = self.insts[i as usize].next;
        }
        match prev {
            NONE => self.blocks[bi].first = NONE,
            p => self.insts[p as usize].next = NONE,
        }
        let blk = &mut self.blocks[bi];
        blk.last = prev;
        blk.len -= 1;
        self.cur = bi as u32;
        self.var_cur.iter_mut().for_each(|x| *x = NONE);
        last
    }

    /// Puts instruction `i`, taken off by [`Lir::reopen`], back at the end
    /// of the current block.
    pub(in crate::jit_backend::trace) fn close(&mut self, i: u32) {
        let blk = &mut self.blocks[self.cur as usize];
        let last = std::mem::replace(&mut blk.last, i);
        blk.len += 1;
        match last {
            NONE => blk.first = i,
            l => self.insts[l as usize].next = i,
        }
        self.insts[i as usize].next = NONE;
    }

    /// The value the return `r` of block `b` (taken off by [`Lir::reopen`])
    /// returned, a constant the block defines, known as one again.
    pub(in crate::jit_backend::trace) fn returned(&mut self, b: Block, r: u32) -> Value {
        let x = self.insts[r as usize].a;
        let mut i = self.blocks[b.as_u32() as usize].first;
        while i != NONE {
            let inst = &self.insts[i as usize];
            if let (Op::Iconst(c), true) = (inst.op, inst.dst == x) {
                self.konst[x as usize] = Some(c);
            }
            i = inst.next;
        }
        debug_assert!(
            self.konst[x as usize].is_some(),
            "an exit returns a constant"
        );
        val(x)
    }
}
