//! Parallel moves: block arguments into parameters, and the values a call
//! or an exit hands on into fixed registers.

use super::*;

impl<M: Masm> Gen<'_, M> {
    /// Performs the moves collected in `self.mv[k]` (`k` 1 for floats) as
    /// one parallel move.
    pub(super) fn flush_moves(&mut self, k: usize) {
        let float = k == 1;
        let park = if float {
            Loc::Reg(M::FSCRATCH[1])
        } else {
            Loc::Reg(M::SCRATCH[1])
        };
        let mut mv = std::mem::take(&mut self.mv[k]);
        let mut seq = std::mem::take(&mut self.seq);
        sequence(&mut mv, park, &mut seq);
        for &(d, s) in &seq {
            self.one_move(d, s, float);
        }
        mv.clear();
        self.mv[k] = mv;
        self.seq = seq;
    }

    pub(super) fn one_move(&mut self, d: Loc, s: Src, float: bool) {
        let t = if float { M::FSCRATCH[0] } else { M::SCRATCH[0] };
        let r = match s {
            Src::Loc(Loc::Reg(p)) => p,
            Src::Loc(Loc::Stack(sl)) => {
                let o = self.spill_off(sl);
                if float {
                    self.m.fload(t, M::SP, o);
                } else {
                    self.m.load(Width::B8, t, M::SP, o);
                }
                t
            }
            Src::Loc(Loc::None) => return,
        };
        match d {
            Loc::Reg(p) if float => self.m.fmov(p, r),
            Loc::Reg(p) => self.m.mov(p, r),
            Loc::Stack(sl) => {
                let o = self.spill_off(sl);
                if float {
                    self.m.fstore(r, M::SP, o);
                } else {
                    self.m.store(Width::B8, r, M::SP, o);
                }
            }
            Loc::None => {}
        }
    }

    /// Moves the arguments `args` into block `b`'s parameters.
    pub(super) fn edge_moves(&mut self, b: u32, args: &[u32]) {
        let lir = self.lir;
        let params = lir.block_params(b);
        if params.is_empty() {
            return;
        }
        for (&p, &a) in params.iter().zip(args) {
            let m = (self.loc(p), Src::Loc(self.loc(a)));
            let k = usize::from(self.is_float(p));
            self.mv[k].push(m);
        }
        self.flush_moves(0);
        self.flush_moves(1);
    }
}
