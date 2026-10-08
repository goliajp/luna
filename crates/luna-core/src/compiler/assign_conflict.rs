//! PUC `check_conflict` of a multiple assignment.

use super::lvalue::{KeyRef, Lv, TabRef};
use super::*;

impl Compiler<'_> {
    /// PUC `check_conflict`: target `lv`, a local or upvalue, is assigned
    /// after the indexings `prev` that read it as their table or key; they
    /// get a copy of its value taken now.
    pub(super) fn check_conflict(&mut self, prev: &mut [Lv], lv: Lv) -> Result<(), SyntaxError> {
        let extra = self.lr().freereg;
        let mut conflict = false;
        for p in prev.iter_mut() {
            let Lv::Indexed(t, key) = p else {
                continue;
            };
            match (lv, *t) {
                (Lv::Upval(u), TabRef::Up(tu)) if u == tu => {
                    conflict = true;
                    *t = TabRef::Reg(extra);
                }
                (Lv::Local(r), TabRef::Reg(tr)) if r == tr => {
                    conflict = true;
                    *t = TabRef::Reg(extra);
                }
                _ => {}
            }
            if let (Lv::Local(r), KeyRef::Reg(k)) = (lv, *key)
                && r == k
            {
                conflict = true;
                *key = KeyRef::Reg(extra);
            }
        }
        if conflict {
            match lv {
                Lv::Local(r) => self.emit(Inst::iabc(Op::Move, extra, r, 0, false)),
                Lv::Upval(u) => self.emit(Inst::iabc(Op::GetUpval, extra, u, 0, false)),
                Lv::Indexed(..) => unreachable!("only a variable conflicts"),
            };
            self.reserve(1)?;
        }
        Ok(())
    }
}
