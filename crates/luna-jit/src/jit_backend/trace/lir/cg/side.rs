//! The baseline tier's calling exits, which go on to the shared call stub,
//! and the relink block's way in from it (see `super::super::stub`).

use super::*;
use luna_core::__private::SideCell;

impl<M: Masm> Gen<'_, M> {
    /// A calling exit (`args`: its cell, its return, its note): the return
    /// returned while the cell holds no side trace, else on to the call stub
    /// with what it takes in [`Masm::STUB_ARGS`], the trace's frame left.
    pub(crate) fn stub_exit(&mut self, args: &[u32]) -> Result<(), &'static str> {
        let (lir, s) = (self.lir, M::STUB_ARGS);
        let &[c, ret, noted] = args else {
            unreachable!("a calling exit hands on a cell, a return and a note")
        };
        let rl = lir.relink.expect("a trace with calling exits relinks");
        let konst =
            |x: u32| lir.konst[x as usize].expect("an exit's return and note are constants");
        let stub = super::super::stub::address::<M>()?;
        let empty = self.m.new_label();
        let cell = self.src(c, 0);
        let at = SideCell::offsets().entry;
        self.m.load(Width::B8, M::SCRATCH[1], cell, at);
        self.m.branch_reg(M::SCRATCH[1], false, empty);
        self.mv[0].push((Loc::Reg(s[0]), Src::Loc(self.loc(lir.arg0))));
        self.mv[0].push((Loc::Reg(s[1]), Src::Loc(self.loc(c))));
        self.flush_moves(0);
        self.m.mov_imm(s[2], konst(ret));
        self.m
            .mov_imm(s[3], konst(noted) | i64::from(rl.head_pc) << 32);
        self.m.lea(s[4], s[0], rl.ran_off);
        self.m.lea_label(s[5], self.labels[rl.block as usize]);
        self.m.mov_imm(M::SCRATCH[1], stub as i64);
        self.m.epilogue_jmp(M::SCRATCH[1]);
        self.m.bind(empty);
        let r = self.src(ret, 0);
        self.m.mov(M::RET, r);
        self.m.epilogue_ret();
        Ok(())
    }

    /// The relink block's way in from the call stub: the trace's frame made
    /// again as at the entry, and `reg_state` taken from the first argument
    /// register. Nothing else is live there (see `lower::reentry`).
    pub(crate) fn relink_entry(&mut self, saved: &[u8], fsaved: &[u8], locals: u32) {
        self.m.prologue(saved, fsaved, locals);
        self.write_from(self.lir.arg0, M::INT_ARGS[0]);
    }
}
