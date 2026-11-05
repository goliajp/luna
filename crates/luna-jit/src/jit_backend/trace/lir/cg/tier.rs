//! The baseline tier's count of a loop's passes (see `Op::TierCount`).

use super::*;

impl<M: Masm> Gen<'_, M> {
    /// `TierCount` `i`: adds one to the count, in the trace's count cell or
    /// in this call's word of the register buffer, and goes to the hot
    /// block when it reaches `at`.
    pub(crate) fn tier_count(&mut self, i: &Inst, next: u32) {
        let Op::TierCount { count, at } = i.op else {
            unreachable!("a TierCount")
        };
        let [a, n] = M::SCRATCH;
        let (b, off) = match count {
            CountAt::Cell(cell) => {
                self.m.mov_reloc(a, self.lir.relocs[cell as usize].1, cell);
                (a, 0)
            }
            CountAt::Frame(off) => (self.src(i.a, 0), off),
        };
        self.m.load(Width::B4, n, b, off);
        if !self.m.alu_imm(Alu::Add, false, n, n, 1) {
            unreachable!("adding one always encodes");
        }
        self.m.store(Width::B4, n, b, off);
        if !self.m.cmp_imm(false, n, i64::from(at as i32)) {
            self.m.mov_imm(a, i64::from(at));
            self.m.cmp(false, n, a);
        }
        self.m.jcc(Cond::Eq, self.labels[i.b as usize]);
        if i.c != next {
            self.m.jmp(self.labels[i.c as usize]);
        }
    }
}
