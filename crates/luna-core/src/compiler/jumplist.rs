//! Jump lists as PUC's code generator keeps them (`lcode.c`): the jumps of
//! one list are chained through their own offset fields, the last one
//! holding [`NO_JUMP`]. Before 5.4 the jumps patched to the next
//! instruction wait in `jpc` until it is emitted, and a jump emitted there
//! takes them over; from 5.4 on they are patched at once and a final pass
//! sends every jump to a jump on to its final target.

use super::*;

/// The end of a jump list, and an empty list.
pub(super) const NO_JUMP: i32 = -1;

impl Compiler<'_> {
    /// PUC `getjump`: the next jump of the list after the one at `pc`.
    pub(super) fn get_jump(&self, pc: usize) -> i32 {
        let off = self.lr().code[pc].jump_offset();
        if off == NO_JUMP {
            NO_JUMP
        } else {
            pc as i32 + 1 + off
        }
    }

    /// PUC `fixjump`.
    pub(super) fn fix_jump(&mut self, pc: usize, dest: usize) -> Result<(), SyntaxError> {
        let off = dest as i64 - (pc as i64 + 1);
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        let i = self.lr().code[pc];
        self.l().code[pc] = i.with_jump_offset(off as i32);
        Ok(())
    }

    /// PUC `luaK_concat`: `l2` appended to the list `l1`.
    pub(super) fn concat_list(&mut self, l1: &mut i32, l2: i32) -> Result<(), SyntaxError> {
        if l2 == NO_JUMP {
            return Ok(());
        }
        if *l1 == NO_JUMP {
            *l1 = l2;
            return Ok(());
        }
        let mut list = *l1 as usize;
        loop {
            let next = self.get_jump(list);
            if next == NO_JUMP {
                break;
            }
            list = next as usize;
        }
        self.fix_jump(list, l2 as usize)
    }

    /// PUC `luaK_jump`: a new jump, its target to be fixed; before 5.4 the
    /// jumps waiting for the next instruction join its list.
    pub(super) fn jump(&mut self) -> Result<i32, SyntaxError> {
        let jpc = std::mem::replace(&mut self.l().jpc, NO_JUMP);
        let mut j = self.emit(Inst::isj(Op::Jmp, NO_JUMP)) as i32;
        self.concat_list(&mut j, jpc)?;
        Ok(j)
    }

    /// PUC `luaK_getlabel`: the next pc, marked as a jump target.
    pub(super) fn get_label(&mut self) -> usize {
        let here = self.here();
        self.mark_target(here);
        here
    }

    /// PUC `getjumpcontrol`: the test controlling the jump at `pc`, or the
    /// jump itself.
    fn jump_control(&self, pc: usize) -> usize {
        let code = &self.lr().code;
        if pc >= 1 && code[pc - 1].op().is_test() {
            pc - 1
        } else {
            pc
        }
    }

    /// PUC `patchtestreg`: a `TestSet` controlling the jump at `node` puts
    /// its value in `reg`, or becomes a `Test` when there is none to put (or
    /// it is there already). False when the jump has no `TestSet`.
    fn patch_test_reg(&mut self, node: usize, reg: Option<u32>) -> bool {
        let at = self.jump_control(node);
        let i = self.lr().code[at];
        if i.op() != Op::TestSet {
            return false;
        }
        self.l().code[at] = match reg {
            Some(r) if r != i.b() => Inst::iabc(Op::TestSet, r, i.b(), 0, i.k()),
            _ => Inst::iabc(Op::Test, i.b(), 0, 0, i.k()),
        };
        true
    }

    /// PUC `removevalues`.
    pub(super) fn remove_values(&mut self, mut list: i32) {
        while list != NO_JUMP {
            self.patch_test_reg(list as usize, None);
            list = self.get_jump(list as usize);
        }
    }

    /// PUC `patchlistaux`: jumps whose test produces a value go to
    /// `vtarget` with it in `reg`, the others to `dtarget`.
    pub(super) fn patch_list_aux(
        &mut self,
        mut list: i32,
        vtarget: usize,
        reg: Option<u32>,
        dtarget: usize,
    ) -> Result<(), SyntaxError> {
        while list != NO_JUMP {
            let pc = list as usize;
            let next = self.get_jump(pc);
            if self.patch_test_reg(pc, reg) {
                self.fix_jump(pc, vtarget)?;
            } else {
                self.fix_jump(pc, dtarget)?;
            }
            list = next;
        }
        Ok(())
    }

    /// PUC `luaK_patchlist`.
    pub(super) fn patch_list(&mut self, list: i32, target: usize) -> Result<(), SyntaxError> {
        if self.version <= LuaVersion::Lua53 && target == self.here() {
            return self.patch_to_here(list);
        }
        self.patch_list_aux(list, target, None, target)?;
        self.mark_target(target);
        Ok(())
    }

    /// PUC `luaK_patchtohere`.
    pub(super) fn patch_to_here(&mut self, list: i32) -> Result<(), SyntaxError> {
        let here = self.get_label();
        if self.version <= LuaVersion::Lua53 {
            let mut jpc = self.lr().jpc;
            self.concat_list(&mut jpc, list)?;
            self.l().jpc = jpc;
            return Ok(());
        }
        self.patch_list_aux(list, here, None, here)
    }

    /// PUC `need_value`: some jump of the list has no `TestSet` to give it
    /// a value.
    pub(super) fn need_value(&self, mut list: i32) -> bool {
        while list != NO_JUMP {
            let at = self.jump_control(list as usize);
            if self.lr().code[at].op() != Op::TestSet {
                return true;
            }
            list = self.get_jump(list as usize);
        }
        false
    }

    /// PUC `dischargejpc`, run before an instruction is emitted: the jumps
    /// waiting for it land on it. An error (a jump too long) is kept for
    /// [`Self::jump_error`].
    #[inline(never)]
    pub(super) fn discharge_jpc(&mut self) {
        let jpc = std::mem::replace(&mut self.l().jpc, NO_JUMP);
        if jpc == NO_JUMP {
            return;
        }
        let here = self.here();
        if let Err(e) = self.patch_list_aux(jpc, here, None, here) {
            self.l().jump_err.get_or_insert(e);
        }
    }

    /// The error [`Self::discharge_jpc`] met, raised at the next statement.
    pub(super) fn jump_error(&mut self) -> Result<(), SyntaxError> {
        match self.l().jump_err.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// PUC 5.2 / 5.3 `luaK_patchclose`: every jump of the list also closes
    /// the upvalues from register `level` on.
    pub(super) fn patch_close(&mut self, mut list: i32, level: u32) {
        while list != NO_JUMP {
            let pc = list as usize;
            let next = self.get_jump(pc);
            let i = self.lr().code[pc];
            self.l().code[pc] = i.with_close(level + 1);
            list = next;
        }
    }

    /// PUC 5.4+ `luaK_finish` for jumps: a jump to a jump goes to where
    /// the chain of jumps ends.
    pub(super) fn finish_jumps(&mut self) -> Result<(), SyntaxError> {
        if self.version < LuaVersion::Lua54 {
            return Ok(());
        }
        for pc in 0..self.here() {
            if self.lr().code[pc].op() != Op::Jmp {
                continue;
            }
            let mut target = pc;
            for _ in 0..100 {
                let i = self.lr().code[target];
                if i.op() != Op::Jmp {
                    break;
                }
                target = (target as i64 + 1 + i.sj() as i64) as usize;
            }
            self.fix_jump(pc, target)?;
        }
        Ok(())
    }
}
