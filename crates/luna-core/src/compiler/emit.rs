//! Instruction emission, jump patching, register reservation and constants.

use super::*;

impl<'a> Compiler<'a> {
    pub(super) fn emit(&mut self, i: Inst) -> usize {
        let line = self.force_line.unwrap_or(self.last_line);
        let l = self.l();
        l.code.push(i);
        l.lines.push(line);
        l.code.len() - 1
    }

    pub(super) fn emit_jump(&mut self) -> usize {
        self.emit(Inst::isj(Op::Jmp, 0))
    }

    pub(super) fn here(&self) -> usize {
        self.lr().code.len()
    }

    /// PUC ≤5.3's OP_JMP used an 18-bit sBx field, capping reachable code
    /// at ~131k instructions. luna's bytecode widens that to 24-bit sJ
    /// (~16M), but the 5.3 test gate (constructs.lua :308) literally
    /// generates a 262144-instruction `while` to probe the limit. Match
    /// the older dialects' tighter cap so the error fires when the suite
    /// expects it. 5.4+ keeps the wider luna bound.
    pub(super) fn jump_cap(&self) -> u64 {
        if self.version <= LuaVersion::Lua53 {
            (1u64 << 17) - 1
        } else {
            MAX_SJ as u64
        }
    }

    /// Patch a pending forward jump emitted earlier at `pc` so that it lands
    /// at the current `here()` position, and mark `here()` as a jump target
    /// (see `Level::last_target`). Mirrors PUC `luaK_patchtohere`.
    ///
    /// This is the canonical "this jump lands at the next instruction we are
    /// about to emit" hook; every patch-pending-forward-jump call site routes
    /// through it so that the jump-target tracker stays consistent. There is
    /// no separate `patch_jump` variant that elides the mark — patching a
    /// forward jump to a position that is not yet a target is meaningless.
    pub(super) fn patch_to_here(&mut self, pc: usize) -> Result<(), SyntaxError> {
        let target = self.here();
        let off = target as i64 - pc as i64 - 1;
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.l().code[pc].set_sj(off as i32);
        self.mark_target(target);
        Ok(())
    }

    /// Point the jump at `pc` back to `target`, an earlier pc.
    pub(super) fn patch_back(&mut self, pc: usize, target: usize) -> Result<(), SyntaxError> {
        let off = target as i64 - pc as i64 - 1;
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.l().code[pc].set_sj(off as i32);
        self.mark_target(target);
        Ok(())
    }

    pub(super) fn jump_back(&mut self, target: usize) -> Result<(), SyntaxError> {
        let off = target as i64 - self.here() as i64 - 1;
        if off.unsigned_abs() > self.jump_cap() {
            return Err(self.err(self.last_line, "control structure too long"));
        }
        self.emit(Inst::isj(Op::Jmp, off as i32));
        // The back-edge lands at `target`, which was captured upstream
        // (typically `let top = self.here()` before a loop header). Mark it
        // so a future peephole pass sees that pc as occupied.
        self.mark_target(target);
        Ok(())
    }

    /// Record that `pc` is now a jump destination. Monotonic; advances
    /// `last_target` only when `pc` exceeds the recorded maximum. Mirrors the
    /// effect of PUC `luaK_getlabel` (which sets `fs->lasttarget = fs->pc`).
    pub(super) fn mark_target(&mut self, pc: usize) {
        let l = self.l();
        match l.last_target {
            None => l.last_target = Some(pc),
            Some(t) if pc > t => l.last_target = Some(pc),
            _ => {}
        }
    }

    /// Whether the instruction at `here() - 1` may take the place of a Move
    /// that would otherwise be emitted at `here()`: every path that would
    /// reach the Move then runs that instruction last. `false` when nothing
    /// has been emitted yet, or when a jump lands at `here()` (such a path
    /// skips the instruction and needs the Move).
    ///
    /// A jump landing at `here() - 1` itself is fine (PUC `discharge2reg`
    /// rewrites the A field without looking at `fs->lasttarget`): the paths
    /// arriving there run the rewritten instruction like the fall-through
    /// path does, and the temporary register it wrote is read only by the
    /// Move being dropped.
    ///
    /// Consumed by the Reloc-landing peephole at `assign_name` and the
    /// RHS materialization elision at `assign_stat`.
    pub(super) fn no_jump_lands_here(&self) -> bool {
        let here = self.here();
        if here == 0 {
            return false;
        }
        match self.lr().last_target {
            None => true,
            Some(t) => t < here,
        }
    }

    /// Reloc-landing peephole gate. Returns `Some(prev_pc)` when the
    /// instruction at `here() - 1` is a retargetable producer whose A field
    /// equals `vreg` AND no jump lands right after it. The caller can
    /// then `patch_dest(prev_pc, local_reg)` to retarget the A field
    /// directly and skip the otherwise-required `Move local_reg, vreg`.
    ///
    /// The "retargetable producer" set is the closed list of ops produced
    /// by paths that yield `Exp::Reloc(pc)`: arith / bitwise / unop / Len /
    /// Get{Field,I,Table,TabUp}. Concat / SelfOp / Move are NOT in the set
    /// (Concat reads A as operand base, SelfOp writes A+1 too, Move's A is
    /// a sink). LoadK / LoadI / LoadF / LoadNil are excluded because they
    /// are already discharged to their final register by `exp_to_reg` —
    /// no Reloc landing happens through assign_name for them.
    ///
    pub(super) fn assign_name_can_retarget_reloc(&self, vreg: u32) -> Option<usize> {
        if !self.no_jump_lands_here() {
            return None;
        }
        let prev_pc = self.here() - 1;
        let prev = self.lr().code[prev_pc];
        if !is_retargetable_op(prev.op()) {
            return None;
        }
        if prev.a() != vreg {
            return None;
        }
        // The value may sit in a local's own register (`assign_stat` stores
        // `b = a` from `a` directly): the instruction before is then the
        // statement that last assigned `a`, and retargeting it would drop
        // that assignment.
        if self
            .lr()
            .locals
            .iter()
            .any(|v| v.konst.is_none() && v.reg == vreg)
        {
            return None;
        }
        Some(prev_pc)
    }

    /// PUC `errorlimit`: render the "too many … (limit is …) in <where>"
    /// message a per-function-cap check raises. `where` is "main function"
    /// for the chunk's top-level proto and "function at line N" for every
    /// nested function — N is the proto's `line_defined`. errors.lua :766/:775
    /// check the line number substring.
    pub(super) fn limit_err(&self, what: &str, limit: u32) -> SyntaxError {
        self.limit_err_at(self.levels.len() - 1, what, limit)
    }

    /// PUC's "too many X" errors attribute to the *level* whose budget got
    /// exhausted (`L->ci`'s `func`'s `linedefined`), which is not necessarily
    /// the currently-being-compiled function when an upvalue cascade walks up
    /// the lexical chain. 5.1 errors.lua :238's 70 nested closures fill foo1's
    /// upval cap when foo70 first references a70, and PUC reports
    /// "...function at line 3" — foo1's start.
    pub(super) fn limit_err_at(&self, li: usize, what: &str, limit: u32) -> SyntaxError {
        let line_defined = self.levels[li].line_defined;
        let where_ = if line_defined == 0 {
            "main function".to_string()
        } else {
            format!("function at line {line_defined}")
        };
        let msg = if self.version <= LuaVersion::Lua51 {
            format!("{where_} has more than {limit} {what}")
        } else {
            format!("too many {what} (limit is {limit}) in {where_}")
        };
        self.err(self.last_line, msg)
    }

    /// Upvalues a level already holds that count against the limit. A 5.1
    /// function's slot 0 is the `_ENV` cell luna adds for `setfenv`; PUC 5.1
    /// keeps a function's environment outside its upvalues, so that slot is
    /// not one of the 60.
    pub(super) fn counted_upvals(&self, li: usize) -> u32 {
        let n = self.levels[li].upvals.len() as u32;
        let hidden_env = self.version == LuaVersion::Lua51
            && self.levels[li]
                .upvals
                .first()
                .is_some_and(|u| &*u.name == "_ENV");
        n - u32::from(hidden_env)
    }

    /// PUC `luaK_checkstack` on overflow: 5.1/5.2 say the expression is too
    /// complex, 5.3/5.4 that it needs too many registers, 5.5 runs it through
    /// `errorlimit`. PUC appends the token being read; the AST keeps no
    /// tokens, so luna cannot.
    pub(super) fn regs_error(&self, line: u32) -> SyntaxError {
        match self.version {
            LuaVersion::Lua51 | LuaVersion::Lua52 => {
                self.err(line, "function or expression too complex")
            }
            LuaVersion::Lua55 => self.limit_err("registers", 255),
            _ => self.err(line, "function or expression needs too many registers"),
        }
    }

    pub(super) fn reserve(&mut self, n: u32) -> Result<u32, SyntaxError> {
        let line = self.last_line;
        let cap = max_regs(self.version);
        let l = self.l();
        let base = l.freereg;
        l.freereg += n;
        if l.freereg > cap {
            return Err(self.regs_error(line));
        }
        if l.freereg > l.max_stack {
            l.max_stack = l.freereg;
        }
        Ok(base)
    }

    pub(super) fn set_freereg(&mut self, r: u32) {
        let l = self.l();
        l.freereg = r;
        if r > l.max_stack {
            l.max_stack = r;
        }
    }

    pub(super) fn str_const(&mut self, bytes: &[u8]) -> u32 {
        let s = self.intern_str(bytes);
        self.const_idx(ConstKey::Str(s.as_ptr()), Value::Str(s))
    }

    /// The constant of the tree's string (or name) `s`: each entry of the
    /// chunk's names is interned on the heap once per load.
    pub(super) fn sym_const(&mut self, s: ast::Sym) -> u32 {
        let i = s.0 as usize;
        let g = match self.sym_strs[i] {
            Some(g) => g,
            None => {
                let g = self.intern_str(self.sb(s));
                self.sym_strs[i] = Some(g);
                g
            }
        };
        self.const_idx(ConstKey::Str(g.as_ptr()), Value::Str(g))
    }

    pub(super) fn intern_str(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        // intern the literal once per chunk so identical constants share an
        // object; heap.intern already dedups short strings, the cache only
        // has to hold long ones
        if bytes.len() <= crate::runtime::string::MAX_SHORT_LEN {
            self.heap.intern(bytes)
        } else {
            self.long_str(bytes)
        }
    }

    pub(super) fn long_str(&mut self, bytes: &[u8]) -> Gc<LuaStr> {
        match self.str_cache.get(bytes) {
            Some(s) => *s,
            None => {
                let s = self.heap.intern(bytes);
                self.str_cache.insert(bytes.into(), s);
                s
            }
        }
    }

    pub(super) fn load_const(&mut self, reg: u32, c: u32) {
        if c <= MAX_BX {
            self.emit(Inst::iabx(Op::LoadK, reg, c));
        } else {
            self.emit(Inst::iabx(Op::LoadKx, reg, 0));
            self.emit(Inst::iax(Op::ExtraArg, c));
        }
    }

    /// Rewrite the wanted-results field (C) of an open CALL/VARARG.
    pub(super) fn patch_wanted(&mut self, pc: usize, wanted_plus1: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst((i.0 & 0x00FF_FFFF) | (wanted_plus1 << 24));
    }

    /// Rewrite the destination (A) field of a pending instruction.
    pub(super) fn patch_dest(&mut self, pc: usize, reg: u32) {
        let i = self.l().code[pc];
        self.l().code[pc] = Inst(i.0 & !(0xFF << 7) | (reg << 7));
    }
}
