//! Blocks, and the `break`s and `goto`s that leave them, as each dialect's
//! parser handles them (`enterblock`, `leaveblock`, the label and goto
//! lists): 5.1 closes on the spot, 5.2 / 5.3 close in the jump itself,
//! 5.4 closes at the label and 5.5 in a `CLOSE` placed before the jump.
//! The frontend has already checked every goto and label.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn enter_block(&mut self, is_loop: bool) {
        let l = self.lr();
        let b = BlockCx {
            first_local: l.locals.len(),
            first_avar: l.avars.len(),
            reg_floor: l.freereg,
            is_loop: is_loop as u8,
            breaklist: NO_JUMP,
            first_label: l.labels.len(),
            first_goto: l.gotos.len(),
            gdecls: LVec::new(self.heap.mem()),
            collective: None,
            has_tbc: false,
            tbc_scope: false,
        };
        self.l().blocks.push_or_abort(b);
    }

    /// PUC `reglevel`: the registers the first `nactvar` active variables
    /// take.
    pub(super) fn reg_level(&self, nactvar: usize) -> u32 {
        self.lr().avars[..nactvar]
            .iter()
            .rev()
            .find_map(|a| a.reg)
            .map_or(0, |r| r + 1)
    }

    /// PUC `removevars`: the block's locals leave scope here. Its entries of
    /// `avars` stay until the block is left, for [`Self::reg_level`].
    pub(super) fn remove_vars(&mut self, b: &BlockCx<'a>) {
        let end_pc = self.here() as u32;
        let lvl = self.l();
        for l in &lvl.locals[b.first_local..] {
            if l.konst.is_some() {
                continue;
            }
            lvl.locvars.push_or_abort(crate::runtime::LocVar {
                name: l.name.into(),
                reg: l.reg,
                start_pc: l.start_pc,
                end_pc,
            });
        }
        lvl.locals.truncate(b.first_local);
    }

    pub(super) fn leave_block(&mut self) -> Result<(), SyntaxError> {
        let b = self.l().blocks.pop().expect("block underflow");
        // PUC `bl->upval`: a local of the block is captured, or to be closed
        let upval = b.has_tbc || self.lr().locals[b.first_local..].iter().any(|l| l.captured);
        let previous = !self.lr().blocks.is_empty();
        let level = self.reg_level(b.first_avar);
        match self.version {
            LuaVersion::Lua51 => {
                self.remove_vars(&b);
                if upval && previous {
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                }
                self.set_freereg(level);
                self.patch_to_here(b.breaklist)?;
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                if previous && upval {
                    let j = self.jump()?;
                    self.patch_close(j, level);
                    self.patch_to_here(j)?;
                }
                if b.is_loop != 0 {
                    let (here, n) = (self.here() as i32, self.lr().avars.len());
                    self.new_label("break", here, n);
                    self.find_gotos_53(b.first_goto)?;
                }
                self.remove_vars(&b);
                self.set_freereg(level);
                self.l().labels.truncate(b.first_label);
                if previous {
                    self.move_gotos_out_53(&b, upval)?;
                }
            }
            LuaVersion::Lua54 => {
                self.remove_vars(&b);
                let closed = b.is_loop != 0 && self.create_label_54("break", b.first_avar, &b)?;
                if !closed && previous && upval {
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                }
                self.set_freereg(level);
                self.l().labels.truncate(b.first_label);
                if previous {
                    let lvl = self.l();
                    for g in &mut lvl.gotos[b.first_goto..] {
                        g.close |= upval && level_of(&lvl.avars, g.nactvar) > level;
                        g.nactvar = b.first_avar;
                    }
                }
            }
            _ => {
                if previous && upval {
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                }
                self.set_freereg(level);
                self.remove_vars(&b);
                if b.is_loop == 2 {
                    let here = self.get_label() as i32;
                    self.new_label("break", here, b.first_avar);
                }
                self.solve_gotos_55(&b, upval, level)?;
            }
        }
        self.l().avars.truncate(b.first_avar);
        Ok(())
    }

    pub(super) fn new_label(&mut self, name: &'a str, pc: i32, nactvar: usize) -> usize {
        let l = self.l();
        l.labels.push_or_abort(LabelDesc {
            name,
            pc,
            nactvar,
            close: false,
        });
        l.labels.len() - 1
    }

    pub(super) fn new_goto(&mut self, name: &'a str, pc: i32) -> usize {
        let nactvar = self.lr().avars.len();
        let l = self.l();
        l.gotos.push_or_abort(LabelDesc {
            name,
            pc,
            nactvar,
            close: false,
        });
        l.gotos.len() - 1
    }
}

/// [`Compiler::reg_level`] over `avars`.
pub(super) fn level_of(avars: &[AVar<'_>], nactvar: usize) -> u32 {
    avars[..nactvar]
        .iter()
        .rev()
        .find_map(|a| a.reg)
        .map_or(0, |r| r + 1)
}
