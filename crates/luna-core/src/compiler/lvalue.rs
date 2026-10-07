//! Table keys, stored values and assignment targets, as PUC's code
//! generator handles them (`luaK_indexed`, `luaK_exp2RK` / `luaK_exp2K`,
//! `luaK_storevar`, `restassign`'s `check_conflict`): which operands stay
//! constants and which take registers, so that a function uses the
//! registers PUC's does.

use super::*;

/// Where the table of an indexing is.
#[derive(Clone, Copy)]
pub(super) enum TabRef {
    Reg(u32),
    /// an upvalue (5.2+ `GETTABUP` / `SETTABUP`, with a string key)
    Up(u32),
}

/// How the key of an indexing is given.
#[derive(Clone, Copy)]
pub(super) enum KeyRef {
    /// a string constant the `*Field` / `*TabUp` ops take
    Str(u32),
    /// an integer 0..=255 (`GetI` / `SetI`)
    Int(u32),
    /// any other constant, before 5.4 (`GETTABLE` takes an `RK` key)
    K(u32),
    Reg(u32),
}

/// A stored value: a constant (`k` set on the write) or a register.
#[derive(Clone, Copy)]
pub(super) enum Rk {
    K(u32),
    Reg(u32),
}

/// What a value is as an `RK` / `k` operand.
enum KConst {
    Fits(u32),
    /// a constant past the operand's range
    Big(u32),
    No,
}

/// An assignment target.
#[derive(Clone, Copy)]
pub(super) enum Lv {
    Local(u32),
    Upval(u32),
    Indexed(TabRef, KeyRef),
}

impl Compiler<'_> {
    /// PUC `luaK_exp2K` (5.4+) / the constant half of `luaK_exp2RK`.
    fn exp_const(&mut self, e: &Exp) -> KConst {
        let v = match *e {
            Exp::Const(c) if c <= MAX_C => return KConst::Fits(c),
            Exp::Const(c) => return KConst::Big(c),
            Exp::Int(i) => Value::Int(i),
            Exp::Float(mut f) => {
                if f == 0.0 && self.version == LuaVersion::Lua51 {
                    f = *self.l().zero_51.get_or_insert(f);
                }
                Value::Float(f)
            }
            Exp::Nil => Value::Nil,
            Exp::True => Value::Bool(true),
            Exp::False => Value::Bool(false),
            _ => return KConst::No,
        };
        // 5.1 / 5.2 make a constant an `RK` operand only while the table
        // has room for it (`fs->nk <= MAXINDEXRK`, checked before adding);
        // 5.3+ add it and then check its index
        if self.version <= LuaVersion::Lua52 && self.lr().consts.len() > MAX_C as usize {
            return KConst::No;
        }
        let c = self.const_idx(v);
        if c <= MAX_C { KConst::Fits(c) } else { KConst::Big(c) }
    }

    /// `e` as a stored value (PUC `codeABRK` / `luaK_exp2RK`).
    pub(super) fn exp_rk(&mut self, e: Exp) -> Result<Rk, SyntaxError> {
        Ok(match self.exp_const(&e) {
            KConst::Fits(c) => Rk::K(c),
            // 5.3 has turned the value into a constant past the `RK` range,
            // which is then loaded (`LOADK` even for nil and booleans)
            KConst::Big(c) if self.version == LuaVersion::Lua53 => {
                let r = self.reserve(1)?;
                self.load_const(r, c);
                Rk::Reg(r)
            }
            _ => Rk::Reg(self.held_reg(e)?),
        })
    }

    /// `e` in a register that stays reserved: a temporary `Exp::Reg` at or
    /// past `freereg` is taken, as PUC's `VNONRELOC` temporaries are.
    pub(super) fn held_reg(&mut self, e: Exp) -> Result<u32, SyntaxError> {
        let r = self.exp_to_anyreg(e)?;
        if r >= self.lr().freereg {
            self.set_freereg(r + 1);
        }
        Ok(r)
    }

    /// The upvalue `e` was just loaded from by a `GetUpval` that nothing
    /// has used yet: PUC 5.2+ keeps an upvalue table as an upvalue until
    /// it knows the key. The load is taken back.
    pub(super) fn take_upval_load(&mut self, e: &Exp) -> Option<u32> {
        let Exp::Reloc(pc) = *e else {
            return None;
        };
        let i = self.lr().code[pc];
        if self.version < LuaVersion::Lua52 || i.op() != Op::GetUpval || pc + 1 != self.here() {
            return None;
        }
        self.l().code.pop();
        self.l().lines.pop();
        Some(i.b())
    }

    /// Whether constant `c` is a string the field ops take: in 5.4+ only
    /// a short one (PUC `isKstr`).
    fn kstr(&self, c: u32) -> bool {
        c <= MAX_C
            && match self.lr().consts.get(c as usize) {
                Some(Value::Str(s)) => self.version < LuaVersion::Lua54 || s.is_short(),
                _ => false,
            }
    }

    /// PUC `luaK_indexed`: table `t` (a register, or an upvalue kept as
    /// one) indexed by `key`, whose code is compiled but not discharged.
    pub(super) fn indexed(
        &mut self,
        t: TabRef,
        key: Exp,
    ) -> Result<(TabRef, KeyRef), SyntaxError> {
        if let Exp::Const(c) = key
            && self.kstr(c)
        {
            return Ok((t, KeyRef::Str(c)));
        }
        // 5.2 / 5.3 keep an upvalue table whatever the key (`GETTABUP` takes
        // an `RK` key)
        if let TabRef::Up(_) = t
            && matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53)
        {
            return Ok(match self.exp_rk(key)? {
                Rk::K(c) => (t, KeyRef::K(c)),
                Rk::Reg(r) => (t, KeyRef::Reg(r)),
            });
        }
        // an upvalue indexed by anything else goes into a register, after a
        // comparison key has used its operands
        let key = match (t, key) {
            (TabRef::Up(_), Exp::Cmp { .. }) => Exp::Reg(self.held_reg(key)?),
            _ => key,
        };
        let t = match t {
            TabRef::Up(u) => {
                let r = self.reserve(1)?;
                self.emit(Inst::iabc(Op::GetUpval, r, u, 0, false));
                TabRef::Reg(r)
            }
            t => t,
        };
        if self.version < LuaVersion::Lua54 {
            // an `RK` key; a small integer one still takes `GetI` / `SetI`
            return Ok(match self.exp_rk(key)? {
                Rk::K(c) => match key {
                    Exp::Int(i) if (0..=255).contains(&i) => (t, KeyRef::Int(i as u32)),
                    _ => (t, KeyRef::K(c)),
                },
                Rk::Reg(r) => (t, KeyRef::Reg(r)),
            });
        }
        if let Exp::Int(i) = key
            && (0..=255).contains(&i)
        {
            return Ok((t, KeyRef::Int(i as u32)));
        }
        Ok((t, KeyRef::Reg(self.held_reg(key)?)))
    }

    /// The read of an indexing, its destination pending.
    pub(super) fn index_get(&mut self, t: TabRef, key: KeyRef) -> Exp {
        let i = match (t, key) {
            (TabRef::Up(u), KeyRef::Str(c)) => Inst::iabc(Op::GetTabUp, 0, u, c, true),
            (TabRef::Reg(r), KeyRef::Str(c)) => Inst::iabc(Op::GetField, 0, r, c, true),
            (TabRef::Reg(r), KeyRef::Int(c)) => Inst::iabc(Op::GetI, 0, r, c, false),
            (TabRef::Reg(r), KeyRef::K(c)) => Inst::iabc(Op::GetTableK, 0, r, c, false),
            (TabRef::Reg(r), KeyRef::Reg(k)) => Inst::iabc(Op::GetTable, 0, r, k, false),
            (TabRef::Up(u), KeyRef::K(c)) => Inst::iabc(Op::GetTabUpR, 0, u, c, true),
            (TabRef::Up(u), KeyRef::Reg(k)) => Inst::iabc(Op::GetTabUpR, 0, u, k, false),
            (TabRef::Up(_), KeyRef::Int(_)) => unreachable!("an upvalue table keeps an `RK` key"),
        };
        Exp::Reloc(self.emit(i))
    }

    /// The write of an indexing.
    pub(super) fn index_set(&mut self, t: TabRef, key: KeyRef, v: Rk) {
        let (c, k) = match v {
            Rk::K(c) => (c, true),
            Rk::Reg(r) => (r, false),
        };
        let i = match (t, key) {
            (TabRef::Up(u), KeyRef::Str(b)) => Inst::iabc(Op::SetTabUp, u, b, c, k),
            (TabRef::Reg(r), KeyRef::Str(b)) => Inst::iabc(Op::SetField, r, b, c, k),
            (TabRef::Reg(r), KeyRef::Int(b)) => Inst::iabc(Op::SetI, r, b, c, k),
            (TabRef::Reg(r), KeyRef::K(b)) => Inst::iabc(Op::SetTableK, r, b, c, k),
            (TabRef::Reg(r), KeyRef::Reg(b)) => Inst::iabc(Op::SetTable, r, b, c, k),
            (TabRef::Up(u), KeyRef::K(b)) => Inst::iabc(Op::SetTabUpK, u, b, c, k),
            (TabRef::Up(u), KeyRef::Reg(b)) => Inst::iabc(Op::SetTabUpR, u, b, c, k),
            (TabRef::Up(_), KeyRef::Int(_)) => unreachable!("an upvalue table keeps an `RK` key"),
        };
        self.emit(i);
    }

    /// The table of an indexing from its compiled object.
    pub(super) fn index_table(&mut self, oe: Exp) -> Result<TabRef, SyntaxError> {
        Ok(match self.take_upval_load(&oe) {
            Some(u) => TabRef::Up(u),
            None => TabRef::Reg(self.held_reg(oe)?),
        })
    }

    /// PUC `luaK_storevar`: `e` into target `lv`.
    pub(super) fn store(&mut self, lv: Lv, e: Exp) -> Result<(), SyntaxError> {
        match lv {
            Lv::Local(r) => self.exp_to_reg(e, r),
            Lv::Upval(u) => {
                let r = self.exp_to_anyreg(e)?;
                self.emit(Inst::iabc(Op::SetUpval, r, u, 0, false));
                Ok(())
            }
            Lv::Indexed(TabRef::Up(u), KeyRef::K(c)) if self.version == LuaVersion::Lua51 => {
                // a 5.1 global whose name is past constant 255, in the
                // shape the 5.1 writer turns back into `SETGLOBAL`
                let saved = self.lr().freereg;
                let v = self.held_reg(e)?;
                let t = self.reserve(2)?;
                self.emit(Inst::iabc(Op::GetUpval, t, u, 0, false));
                self.load_const(t + 1, c);
                self.emit(Inst::iabc(Op::SetTable, t, t + 1, v, false));
                self.set_freereg(saved);
                Ok(())
            }
            Lv::Indexed(t, key) => {
                // 5.1 `SETGLOBAL` stores a register
                let v = if self.version == LuaVersion::Lua51 && matches!(t, TabRef::Up(_)) {
                    Rk::Reg(self.exp_to_anyreg(e)?)
                } else {
                    self.exp_rk(e)?
                };
                self.index_set(t, key, v);
                Ok(())
            }
        }
    }

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
