//! Table constructors.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn table_ctor(&mut self, id: ExprId, line: u32) -> Result<Exp, SyntaxError> {
        let ast = self.ast;
        let Expr::Table { fields, .. } = ast.expr(id) else {
            unreachable!()
        };
        let fields = ast.list(*fields);
        self.last_line = line;
        // `OP_NEWTABLE` and its register come on the `{`
        self.at(TokRef::First(id, 0));
        let treg = self.reserve(1)?;
        let (mut narr, mut nhash) = (0u32, 0u32);
        for f in fields {
            match f {
                TableField::Item(_) => narr += 1,
                _ => nhash += 1,
            }
        }
        // the size operands are set at the end, once it is known whether
        // the last field is an item left open (PUC `luaK_settablesize`)
        let new_pc = self.emit(Inst::iabc(Op::NewTable, treg, 0, 0, false));
        let mut open_last = false;
        // the items kept in registers before a `SETLIST` stores them: 50
        // (`LFIELDS_PER_FLUSH`); 5.5 goes by the registers left (PUC
        // `maxtostore`: a fifth of them when there are 160, else 10 when
        // there are 80, else 1)
        let fields_per_flush = if self.version >= LuaVersion::Lua55 {
            match 255 - self.lr().freereg {
                n if n >= 160 => n / 5,
                n if n >= 80 => 10,
                _ => 1,
            }
        } else {
            50
        };
        let mut pending = 0u32;
        let mut flushed = 0u32;
        let n_items = fields
            .iter()
            .filter(|f| matches!(f, TableField::Item(_)))
            .count();
        let mut item_idx = 0usize;
        // only the constructor's very last field keeps a call or `...`
        // open; an item followed by keyed fields is cut to one value
        let last_is_item = matches!(fields.last(), Some(TableField::Item(_)));
        for (fi, f) in fields.iter().enumerate() {
            match f {
                TableField::Item(v) => {
                    item_idx += 1;
                    let dst = treg + 1 + pending;
                    self.set_freereg(dst);
                    let e = self.expr(*v)?;
                    // an item goes to its register when the next field
                    // starts (`closelistfield`), the last after the `}`
                    self.at(if fi + 1 < fields.len() {
                        TokRef::After(*v, 1)
                    } else {
                        TokRef::After(id, 0)
                    });
                    // last positional item: calls/varargs stay open
                    if item_idx == n_items
                        && last_is_item
                        && let Exp::Open { pc, base } = e
                    {
                        debug_assert_eq!(base, dst);
                        self.patch_wanted(pc, 0);
                        self.setlist_open(treg, flushed)?;
                        open_last = true;
                        pending = 0;
                        continue;
                    }
                    self.set_freereg(dst);
                    let got = self.exp_to_nextreg(e)?;
                    debug_assert_eq!(got, dst);
                    pending += 1;
                    if pending == fields_per_flush {
                        self.setlist(treg, pending, flushed)?;
                        flushed += pending;
                        pending = 0;
                    }
                }
                TableField::Named(name, v) => {
                    // the key's constant: once the name is read up to 5.3,
                    // from 5.4 once `=` is (`luaK_indexed`)
                    let v54 = self.version >= LuaVersion::Lua54;
                    self.at(TokRef::First(*v, if v54 { 0 } else { -1 }));
                    let c = self.sym_const(name.sym);
                    self.rec_field(treg, Exp::Const(c), *v)?;
                }
                TableField::Keyed(k, v) => {
                    let ke = self.expr(*k)?;
                    // `yindex` gives the key a value on the `]`
                    self.at(TokRef::After(*k, 0));
                    let ke = self.exp_to_val(ke)?;
                    self.rec_field(treg, ke, *v)?;
                }
            }
        }
        self.at(TokRef::After(id, 0));
        if pending > 0 {
            self.setlist(treg, pending, flushed)?;
        }
        let (b, c, k) =
            crate::runtime::table::new_table_operands(self.version, narr - open_last as u32, nhash);
        self.l().code[new_pc] = Inst::iabc(Op::NewTable, treg, b, c, k);
        self.set_freereg(treg + 1);
        Ok(Exp::Reg(treg))
    }

    /// PUC `recfield`: `t[key] = v`, the key placed before `v` is compiled.
    fn rec_field(&mut self, treg: u32, key: Exp, v: ExprId) -> Result<(), SyntaxError> {
        let saved = self.lr().freereg;
        // `luaK_indexed` once `=` is read, the store after the value
        self.at(TokRef::First(v, 0));
        let (t, k) = self.indexed(TabRef::Reg(treg), key)?;
        let ve = self.expr(v)?;
        self.at(TokRef::After(v, 0));
        let rk = self.exp_rk(ve)?;
        self.index_set(t, k, rk);
        self.set_freereg(saved);
        Ok(())
    }

    pub(super) fn setlist(&mut self, treg: u32, n: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, n, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, n, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }

    /// SETLIST with B=0: take items up to the runtime top.
    pub(super) fn setlist_open(&mut self, treg: u32, flushed: u32) -> Result<(), SyntaxError> {
        if flushed <= 0xFF {
            self.emit(Inst::iabc(Op::SetList, treg, 0, flushed, false));
        } else {
            self.emit(Inst::iabc(Op::SetList, treg, 0, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, flushed));
        }
        Ok(())
    }
}
