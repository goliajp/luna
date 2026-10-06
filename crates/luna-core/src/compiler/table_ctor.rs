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
        const FIELDS_PER_FLUSH: u32 = 50;
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
        for f in fields {
            match f {
                TableField::Item(v) => {
                    item_idx += 1;
                    let dst = treg + 1 + pending;
                    if dst >= max_regs(self.version) {
                        return Err(self.regs_error(line));
                    }
                    self.set_freereg(dst);
                    let e = self.expr(*v)?;
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
                    if pending == FIELDS_PER_FLUSH {
                        self.setlist(treg, pending, flushed)?;
                        flushed += pending;
                        pending = 0;
                    }
                }
                TableField::Named(name, v) => {
                    let saved = self.lr().freereg;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    let c = self.sym_const(name.sym);
                    if c <= 0xFF {
                        self.emit(Inst::iabc(Op::SetField, treg, c, vr, true));
                    } else {
                        let kr = self.reserve(1)?;
                        self.load_const(kr, c);
                        self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    }
                    self.set_freereg(saved);
                }
                TableField::Keyed(k, v) => {
                    let saved = self.lr().freereg;
                    let ke = self.expr(*k)?;
                    let kr = self.exp_to_anyreg(ke)?;
                    let ve = self.expr(*v)?;
                    let vr = self.exp_to_anyreg(ve)?;
                    self.emit(Inst::iabc(Op::SetTable, treg, kr, vr, false));
                    self.set_freereg(saved);
                }
            }
        }
        if pending > 0 {
            self.setlist(treg, pending, flushed)?;
        }
        let (b, c, k) =
            crate::runtime::table::new_table_operands(self.version, narr - open_last as u32, nhash);
        self.l().code[new_pc] = Inst::iabc(Op::NewTable, treg, b, c, k);
        self.set_freereg(treg + 1);
        Ok(Exp::Reg(treg))
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
