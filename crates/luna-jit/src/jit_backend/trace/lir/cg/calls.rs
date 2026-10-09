//! Calls: arguments into registers and the outgoing stack area.

use super::*;

impl<M: Masm> Gen<'_, M> {
    /// A call: arguments into the argument registers, the result out.
    pub(crate) fn call(&mut self, i: &Inst, addr: Option<usize>, params: &[Ty]) {
        let args = &self.lir.args[i.args_at as usize..(i.args_at + i.n_args) as usize];
        if addr.is_none() {
            let r = self.src(i.a, 0);
            self.m.mov(M::CALL_TARGET, r);
        }
        let (mut ni, mut nf, mut ns) = (0, 0, 0);
        for (k, (&a, &t)) in args.iter().zip(params).enumerate() {
            let s = self.loc(a);
            if M::POSITIONAL_ARGS {
                (ni, nf) = (k, k);
            }
            let reg = if t.is_float() {
                nf += 1;
                M::FLOAT_ARGS.get(nf - 1)
            } else {
                ni += 1;
                M::INT_ARGS.get(ni - 1)
            };
            match reg {
                Some(&r) => self.mv[usize::from(t.is_float())].push((Loc::Reg(r), Src::Loc(s))),
                // stored before the register moves, which may overwrite
                // the registers these arguments are read from
                None => {
                    self.store_arg(s, t.is_float(), M::CALL_SHADOW as i32 + 8 * ns);
                    ns += 1;
                }
            }
        }
        self.flush_moves(0);
        self.flush_moves(1);
        match addr {
            Some(a) => self.m.call_abs(a),
            None => self.m.call_reg(M::CALL_TARGET),
        }
        if i.dst != NONE {
            if self.is_float(i.dst) {
                let d = self.fdst(i.dst, 0);
                self.m.fmov(d, M::FRET);
                self.fcommit(i.dst, d);
            } else {
                self.write_from(i.dst, M::RET);
            }
        }
    }

    /// How many arguments of a call with these parameter types go on the
    /// stack, one 8-byte slot each; `None` on Apple's aarch64 ABI when one of
    /// them is narrower than 8 bytes (it packs those).
    pub(crate) fn stack_args(params: &[Ty]) -> Option<u32> {
        let (mut ni, mut nf, mut ns) = (0, 0, 0);
        for (k, &t) in params.iter().enumerate() {
            if M::POSITIONAL_ARGS {
                (ni, nf) = (k, k);
            }
            let in_reg = if t.is_float() {
                nf += 1;
                nf <= M::FLOAT_ARGS.len()
            } else {
                ni += 1;
                ni <= M::INT_ARGS.len()
            };
            if !in_reg {
                if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) && t.bits() != 64 {
                    return None;
                }
                ns += 1;
            }
        }
        Some(ns)
    }

    /// Stores the argument at `s` to the outgoing slot at `sp + off`.
    fn store_arg(&mut self, s: Loc, float: bool, off: i32) {
        let r = match s {
            Loc::Reg(p) => p,
            Loc::Stack(sl) => {
                let o = self.spill_off(sl);
                if float {
                    self.m.fload(M::FSCRATCH[0], M::SP, o);
                    M::FSCRATCH[0]
                } else {
                    self.m.load(Width::B8, M::SCRATCH[0], M::SP, o);
                    M::SCRATCH[0]
                }
            }
            Loc::None => return,
        };
        if float {
            self.m.fstore(r, M::SP, off);
        } else {
            self.m.store(Width::B8, r, M::SP, off);
        }
    }
}
