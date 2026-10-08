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
    fn remove_vars(&mut self, b: &BlockCx<'a>) {
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

    fn new_label(&mut self, name: &'a str, pc: i32, nactvar: usize) -> usize {
        let l = self.l();
        l.labels.push_or_abort(LabelDesc {
            name,
            pc,
            nactvar,
            close: false,
        });
        l.labels.len() - 1
    }

    fn new_goto(&mut self, name: &'a str, pc: i32) -> usize {
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

    /// PUC `closegoto` / `solvegoto`: goto `g` jumps to `label`, and leaves
    /// the pending list.
    fn solve_goto(&mut self, g: usize, label: LabelDesc<'a>) -> Result<(), SyntaxError> {
        let gt = self.l().gotos.remove(g);
        debug_assert!(
            gt.nactvar >= label.nactvar,
            "the frontend checks goto scopes"
        );
        self.patch_list(gt.pc, label.pc as usize)
    }

    /// 5.2 / 5.3 `findgotos`: the block's pending gotos to the label just
    /// made, the last of `labels`.
    fn find_gotos_53(&mut self, first_goto: usize) -> Result<(), SyntaxError> {
        let label = *self.lr().labels.last().expect("a label");
        let mut i = first_goto;
        while i < self.lr().gotos.len() {
            if self.lr().gotos[i].name == label.name {
                self.solve_goto(i, label)?;
            } else {
                i += 1;
            }
        }
        Ok(())
    }

    /// 5.2 / 5.3 `findlabel`: goto `g` to a label of the current block, when
    /// there is one.
    fn find_label_53(&mut self, g: usize) -> Result<bool, SyntaxError> {
        let b = self.lr().blocks.last().expect("a block");
        let (first_label, first_local, has_tbc) = (b.first_label, b.first_local, b.has_tbc);
        let gt = self.lr().gotos[g];
        let found = self.lr().labels[first_label..]
            .iter()
            .find(|l| l.name == gt.name)
            .copied();
        let Some(label) = found else {
            return Ok(false);
        };
        let upval = has_tbc || self.lr().locals[first_local..].iter().any(|l| l.captured);
        let has_labels = self.lr().labels.len() > first_label;
        if gt.nactvar > label.nactvar && (upval || has_labels) {
            let level = self.reg_level(label.nactvar);
            self.patch_close(gt.pc, level);
        }
        self.solve_goto(g, label)?;
        Ok(true)
    }

    /// 5.2 / 5.3 `movegotosout`: the pending gotos of the block just left
    /// go to the enclosing one, closing what they leave.
    fn move_gotos_out_53(&mut self, b: &BlockCx<'a>, upval: bool) -> Result<(), SyntaxError> {
        let level = self.reg_level(b.first_avar);
        let mut i = b.first_goto;
        while i < self.lr().gotos.len() {
            let gt = self.lr().gotos[i];
            if gt.nactvar > b.first_avar {
                if upval {
                    self.patch_close(gt.pc, level);
                }
                self.l().gotos[i].nactvar = b.first_avar;
            }
            if !self.find_label_53(i)? {
                i += 1;
            }
        }
        Ok(())
    }

    /// 5.4 `createlabel`: a label here at `nactvar` active variables, the
    /// gotos of block `b` to it solved, and a `CLOSE` when one of them needs
    /// it (true then).
    fn create_label_54(
        &mut self,
        name: &'a str,
        nactvar: usize,
        b: &BlockCx<'a>,
    ) -> Result<bool, SyntaxError> {
        let pc = self.get_label() as i32;
        let li = self.new_label(name, pc, nactvar);
        let label = self.lr().labels[li];
        let mut close = false;
        let mut i = b.first_goto;
        while i < self.lr().gotos.len() {
            if self.lr().gotos[i].name == name {
                close |= self.lr().gotos[i].close;
                self.solve_goto(i, label)?;
            } else {
                i += 1;
            }
        }
        if close {
            let level = self.nvarstack();
            self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
        }
        Ok(close)
    }

    /// 5.5 `solvegotos`: each pending goto of block `b` either jumps to a
    /// label of `b` (closing first when it must) or goes to the enclosing
    /// block.
    fn solve_gotos_55(
        &mut self,
        b: &BlockCx<'a>,
        upval: bool,
        out: u32,
    ) -> Result<(), SyntaxError> {
        let mut i = b.first_goto;
        while i < self.lr().gotos.len() {
            let gt = self.lr().gotos[i];
            let found = self.lr().labels[b.first_label..]
                .iter()
                .find(|l| l.name == gt.name)
                .copied();
            match found {
                Some(label) => {
                    if gt.close || label.nactvar < gt.nactvar && upval {
                        // the placeholder `CLOSE` after the jump changes
                        // places with it
                        let level = self.reg_level(label.nactvar);
                        let pc = gt.pc as usize;
                        let lvl = self.l();
                        lvl.code[pc + 1] = lvl.code[pc];
                        lvl.code[pc] = Inst::iabc(Op::Close, level, 0, 0, false);
                        lvl.gotos[i].pc += 1;
                    }
                    let gt = self.l().gotos.remove(i);
                    self.patch_list(gt.pc, label.pc as usize)?;
                }
                None => {
                    let lvl = self.l();
                    if upval && level_of(&lvl.avars, gt.nactvar) > out {
                        lvl.gotos[i].close = true;
                    }
                    lvl.gotos[i].nactvar = b.first_avar;
                    i += 1;
                }
            }
        }
        self.l().labels.truncate(b.first_label);
        Ok(())
    }

    /// A label statement. `last`: only labels follow it in its block, whose
    /// locals it is then out of the scope of.
    pub(super) fn define_label(&mut self, name: &'a str, last: bool) -> Result<(), SyntaxError> {
        let b = self.lr().blocks.last().expect("a block");
        let (first_avar, first_goto) = (b.first_avar, b.first_goto);
        let nactvar = if last {
            first_avar
        } else {
            self.lr().avars.len()
        };
        match self.version {
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let pc = self.get_label() as i32;
                self.new_label(name, pc, nactvar);
                self.find_gotos_53(first_goto)
            }
            LuaVersion::Lua54 => {
                let b = self.l().blocks.pop().expect("a block");
                let r = self.create_label_54(name, nactvar, &b);
                self.l().blocks.push_or_abort(b);
                r.map(|_| ())
            }
            _ => {
                let pc = self.get_label() as i32;
                self.new_label(name, pc, nactvar);
                Ok(())
            }
        }
    }

    pub(super) fn goto_stat(&mut self, name: &'a str, line: u32) -> Result<(), SyntaxError> {
        self.jump_line(line);
        match self.version {
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let j = self.jump()?;
                self.goto_list_53(name, j)
            }
            LuaVersion::Lua54 => {
                let found = self
                    .lr()
                    .labels
                    .iter()
                    .rev()
                    .find(|l| l.name == name)
                    .copied();
                match found {
                    None => {
                        let j = self.jump()?;
                        self.new_goto(name, j);
                    }
                    Some(label) => {
                        let level = self.reg_level(label.nactvar);
                        if self.nvarstack() > level {
                            self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                        }
                        let j = self.jump()?;
                        self.patch_list(j, label.pc as usize)?;
                    }
                }
                Ok(())
            }
            _ => {
                self.goto_55(name)?;
                Ok(())
            }
        }
    }

    /// 5.2 / 5.3 `gotostat` of the jump list `list`.
    fn goto_list_53(&mut self, name: &'a str, list: i32) -> Result<(), SyntaxError> {
        let g = self.new_goto(name, list);
        self.find_label_53(g)?;
        Ok(())
    }

    /// 5.5 `newgotoentry`: the jump and a placeholder `CLOSE` after it.
    fn goto_55(&mut self, name: &'a str) -> Result<usize, SyntaxError> {
        let j = self.jump()?;
        self.emit(Inst::iabc(Op::Close, 0, 1, 0, false));
        Ok(self.new_goto(name, j))
    }

    /// The line of the jump of a `break` / `goto` on `line`: 5.2 / 5.3 emit
    /// it before reading the keyword, so it keeps the line of the token
    /// before.
    fn jump_line(&mut self, line: u32) {
        if !matches!(self.version, LuaVersion::Lua52 | LuaVersion::Lua53) {
            self.last_line = line;
        }
    }

    /// `break`.
    pub(super) fn break_stat(&mut self, line: u32) -> Result<(), SyntaxError> {
        self.jump_line(line);
        match self.version {
            LuaVersion::Lua51 => {
                // the blocks left up to the loop: a CLOSE when one of them
                // has a captured local
                let mut upval = false;
                let mut li = None;
                for (i, b) in self.lr().blocks.iter().enumerate().rev() {
                    if b.is_loop != 0 {
                        li = Some(i);
                        break;
                    }
                    upval |= self.lr().locals[b.first_local..].iter().any(|l| l.captured);
                }
                let li = li.expect("the frontend checks break");
                if upval {
                    let level = self.reg_level(self.lr().blocks[li].first_avar);
                    self.emit(Inst::iabc(Op::Close, level, 0, 0, false));
                }
                let j = self.jump()?;
                let mut list = self.lr().blocks[li].breaklist;
                self.concat_list(&mut list, j)?;
                self.l().blocks[li].breaklist = list;
                Ok(())
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let j = self.jump()?;
                self.goto_list_53("break", j)
            }
            LuaVersion::Lua54 => {
                let j = self.jump()?;
                self.new_goto("break", j);
                Ok(())
            }
            _ => {
                let b = self.l().blocks.iter_mut().rev().find(|b| b.is_loop != 0);
                b.expect("the frontend checks break").is_loop = 2;
                self.goto_55("break")?;
                Ok(())
            }
        }
    }

    /// 5.2–5.4 `test_then_block` of `if cond then break` (5.2 / 5.3 also
    /// `goto`): the condition's true list is the jump. The block it opens is
    /// left to the caller.
    pub(super) fn cond_break(&mut self, t: i32, target: &'a str) -> Result<(), SyntaxError> {
        if self.version == LuaVersion::Lua54 {
            self.new_goto("break", t);
            return Ok(());
        }
        self.goto_list_53(target, t)
    }
}

/// [`Compiler::reg_level`] over `avars`.
fn level_of(avars: &[AVar<'_>], nactvar: usize) -> u32 {
    avars[..nactvar]
        .iter()
        .rev()
        .find_map(|a| a.reg)
        .map_or(0, |r| r + 1)
}
