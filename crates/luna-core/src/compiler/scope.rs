//! Blocks, labels and `goto`.

use super::*;
use crate::runtime::mem::LVec;

impl<'a> Compiler<'a> {
    pub(super) fn enter_block(&mut self, is_loop: bool) {
        let floor = self.lr().freereg;
        let first = self.lr().locals.len();
        let first_avar = self.lr().avars.len();
        let start_pc = self.lr().code.len();
        let mem = self.heap.mem();
        self.l().blocks.push_or_abort(BlockCx {
            first_local: first,
            first_avar,
            reg_floor: floor,
            is_loop,
            breaks: LVec::new(mem),
            break_levels: LVec::new(mem),
            break_close: false,
            start_pc,
            labels: LVec::new(mem),
            gotos: LVec::new(mem),
            gdecls: LVec::new(mem),
            collective: None,
            has_tbc: false,
            tbc_scope: false,
            body_end: None,
            for_loop: false,
            end_line: None,
        });
    }

    pub(super) fn leave_block(&mut self) -> Result<(), SyntaxError> {
        self.leave_block_with(true)
    }

    /// End the innermost block; `close` is false for a function's outermost
    /// block after its final return, which closes the upvalues itself.
    pub(super) fn leave_block_with(&mut self, close: bool) -> Result<(), SyntaxError> {
        let b = self.l().blocks.pop().expect("block underflow");
        let captured = self.lr().locals[b.first_local..].iter().any(|l| l.captured);
        // a `for` loop's variables were closed on each pass
        let open_until = match b.body_end {
            Some((first, _)) if b.for_loop => first,
            _ => usize::MAX,
        };
        let captured_here = self.lr().locals[b.first_local..]
            .iter()
            .enumerate()
            .any(|(i, l)| l.captured && b.first_local + i < open_until);
        // Where the block's CLOSE falls against its locals' `end_pc` is
        // visible to a `__close` handler reading the frame with
        // `debug.getlocal` (`getlocalname` tests `pc < end_pc`). 5.4's
        // `leaveblock` removes the variables first, so the handler finds
        // "(temporary)"; 5.5 closes while they are still in scope (5.5.1
        // locals.lua :1198 pins this for a `repeat` body).
        let v54 = self.version == LuaVersion::Lua54;
        let before_close = self.lr().code.len() as u32;
        // 5.4 `break` is a goto to a label placed here, where the loop's
        // variables are gone; a CLOSE follows the label when some break
        // left active locals of a block with upvalues or to-be-closed
        // variables (PUC `movegotosout` sets the goto's `close`)
        let mut break_close = b.break_close;
        if v54 && (captured || b.has_tbc) {
            if b.is_loop {
                break_close |= b.break_levels.iter().any(|&n| n > b.first_local);
            } else if let Some(lp) = self.l().blocks.iter_mut().rev().find(|x| x.is_loop) {
                let crossed = lp
                    .breaks
                    .iter()
                    .zip(&lp.break_levels)
                    .any(|(&pc, &n)| pc >= b.start_pc && n > b.first_local);
                lp.break_close |= crossed;
            }
        }
        if break_close && let Some(line) = b.end_line {
            self.last_line = line;
        }
        if v54 {
            for &pc in &b.breaks {
                self.patch_to_here(pc)?;
            }
        }
        if close && (captured_here || b.has_tbc || break_close) {
            self.emit(Inst::iabc(Op::Close, b.reg_floor, 0, 0, false));
        }
        // record debug LocVar entries for the locals leaving scope here
        let end_pc = if self.vars_end_before_close() {
            before_close
        } else {
            self.lr().code.len() as u32
        };
        let first_local = b.first_local;
        let lvl = self.l();
        for i in first_local..lvl.locals.len() {
            let l = &lvl.locals[i];
            if l.konst.is_some() {
                continue;
            }
            let rec = crate::runtime::LocVar {
                name: l.name.into(),
                reg: l.reg,
                start_pc: l.start_pc,
                end_pc: match b.body_end {
                    Some((first, pc)) if i >= first => pc,
                    _ => end_pc,
                },
            };
            lvl.locvars.push_or_abort(rec);
        }
        self.l().locals.truncate(b.first_local);
        self.l().avars.truncate(b.first_avar);
        self.set_freereg(b.reg_floor);
        if !v54 {
            for &pc in b.breaks.iter() {
                self.patch_to_here(pc)?;
            }
        }
        // propagate unmatched gotos to the enclosing block (the label may
        // appear after this block); a goto leaving a block with captured or
        // to-be-closed locals must run CLOSE first — route it through a
        // trampoline
        if !b.gotos.is_empty() {
            let mut gotos = b.gotos;
            if captured || b.has_tbc {
                // each distinct label name gets its own close trampoline
                let mut names: LVec<&'a str> = LVec::new(self.heap.mem());
                for g in gotos.iter() {
                    if !names.contains(&g.name) {
                        names.push_or_abort(g.name);
                    }
                }
                let skip = self.emit_jump();
                let mut routed = LVec::with_capacity_or_abort(self.heap.mem(), names.len());
                for &name in names.iter() {
                    let tramp = self.here();
                    self.emit(Inst::iabc(Op::Close, b.reg_floor, 0, 0, false));
                    let new_jmp = self.emit_jump();
                    for g in gotos.iter().filter(|g| g.name == name) {
                        let off = tramp as i64 - g.jmp_pc as i64 - 1;
                        if off.unsigned_abs() > MAX_SJ as u64 {
                            return Err(self.err(g.line, "control structure too long"));
                        }
                        self.l().code[g.jmp_pc].set_sj(off as i32);
                    }
                    // every goto in this batch lands at `tramp` (start of the
                    // per-name close trampoline)
                    self.mark_target(tramp);
                    let line = gotos
                        .iter()
                        .find(|g| g.name == name)
                        .map(|g| g.line)
                        .expect("goto exists");
                    routed.push_or_abort(GotoRef {
                        name,
                        jmp_pc: new_jmp,
                        line,
                        nactive: b.first_avar,
                    });
                }
                self.patch_to_here(skip)?;
                gotos = routed;
            }
            let cap = self.lr().avars.len();
            // PUC `movegotosout` matches each propagated goto against the
            // *immediate* enclosing block's already-defined labels (its
            // `findlabel`) — a forward `goto name` resolved entirely after the
            // block closed would otherwise sit unresolved forever once luna
            // stopped searching ancestor blocks at goto time. math.lua 5.4
            // :995 (`::doagain::` defined ahead, `goto doagain` issued from
            // inside a nested `if`) is the prototypical case. Only the
            // immediate parent is consulted; deeper ancestors are reached
            // later as the parent itself leaves. When the goto jumps over a
            // captured local declared *between* the parent's label and the
            // goto (PUC `luaK_patchclose`), the resolution routes through a
            // CLOSE-and-jump trampoline so those upvalues are properly closed
            // — goto.lua 5.4 :203's foo() backward `goto l1` exercises this.
            let mut unresolved = LVec::with_capacity_or_abort(self.heap.mem(), gotos.len());
            for &g in gotos.iter() {
                let target = self.lr().blocks.last().and_then(|p| {
                    p.labels
                        .iter()
                        .rev()
                        .find(|l| l.name == g.name)
                        .map(|l| (l.pc, l.nactive))
                });
                match target {
                    Some((pc, label_nactive)) => {
                        let needs_close = g.nactive > label_nactive
                            && self.reg_floor_from_avar(label_nactive).is_some();
                        let dest = if needs_close {
                            let skip = self.emit_jump();
                            let tramp = self.here();
                            if let Some(floor) = self.reg_floor_from_avar(label_nactive) {
                                self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
                            }
                            let to_label = pc as i64 - self.here() as i64 - 1;
                            if to_label.unsigned_abs() > MAX_SJ as u64 {
                                return Err(self.err(g.line, "control structure too long"));
                            }
                            self.emit(Inst::isj(Op::Jmp, to_label as i32));
                            self.patch_to_here(skip)?;
                            tramp as i64
                        } else {
                            pc as i64
                        };
                        let off = dest - g.jmp_pc as i64 - 1;
                        if off.unsigned_abs() > MAX_SJ as u64 {
                            return Err(self.err(g.line, "control structure too long"));
                        }
                        self.l().code[g.jmp_pc].set_sj(off as i32);
                        // dest is either a backward label.pc or a trampoline
                        // pc — both are jump destinations.
                        self.mark_target(dest as usize);
                    }
                    None => unresolved.push_or_abort(g),
                }
            }
            match self.l().blocks.last_mut() {
                Some(parent) => {
                    for &g in unresolved.iter() {
                        parent.gotos.push_or_abort(GotoRef {
                            nactive: g.nactive.min(cap),
                            ..g
                        });
                    }
                }
                None if !unresolved.is_empty() => {
                    let g = &unresolved[0];
                    return Err(self.err(
                        g.line,
                        format!(
                            "no visible label '{}' for <goto> at line {}",
                            g.name, g.line
                        ),
                    ));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Define a label here; match pending forward gotos.
    pub(super) fn define_label(
        &mut self,
        name: &'a str,
        line: u32,
        trailing: bool,
    ) -> Result<(), SyntaxError> {
        let here = self.here();
        // active-var count (locals + global decls) at the label: a trailing
        // label sits at the block base (its locals are already out of scope)
        let nactive = if trailing {
            self.lr().blocks.last().expect("no block").first_avar
        } else {
            self.lr().avars.len()
        };
        // PUC 5.2/5.3 `checkrepeated` scoped the duplicate check to the
        // current block — so an inner block could shadow an outer label of
        // the same name (goto.lua 5.2/5.3 :71's `do goto l3; ::l3:: end`
        // alongside an outer `::l3::`). PUC 5.4 widened `findlabel` to scan
        // every label in the function (`fs->firstlabel..n`), making any
        // same-name redeclaration anywhere in the function an error
        // (goto.lua 5.4 :16 `::l1:: do ::l1:: end`). Pick the right scope
        // based on the dialect.
        let dup = if self.version <= LuaVersion::Lua53 {
            self.lr()
                .blocks
                .last()
                .and_then(|b| b.labels.iter().find(|l| l.name == name))
                .map(|l| l.line)
        } else {
            self.lr()
                .blocks
                .iter()
                .flat_map(|b| b.labels.iter())
                .find(|l| l.name == name)
                .map(|l| l.line)
        };
        if let Some(prev_line) = dup {
            return Err(self.err(
                line,
                format!("label '{name}' already defined on line {prev_line}"),
            ));
        }
        let b = self.lr().blocks.last().expect("no block");
        let first_avar = b.first_avar;
        // match pending gotos of this block
        let pending = self.l().blocks.last_mut().expect("no block").gotos.take();
        let mut kept = LVec::with_capacity_or_abort(self.heap.mem(), pending.len());
        for &g in pending.iter() {
            if g.name == name {
                if nactive > g.nactive {
                    // the goto jumps into the scope of the declaration sitting
                    // at its active-var boundary; a `global *` marker has no
                    // name and is reported as '*'. PUC 5.4 says "…scope of
                    // local 'X'"; 5.5 dropped the kind prefix (since `global`
                    // markers can sit on the same chain). luna versions the
                    // wording so the per-dialect test gates can both match.
                    let lname = match &self.lr().avars[g.nactive].name {
                        Some(n) => n.to_string(),
                        None => "*".to_string(),
                    };
                    let kind = if self.version >= LuaVersion::Lua55 {
                        String::new()
                    } else {
                        "local ".to_string()
                    };
                    return Err(self.err(
                        g.line,
                        format!(
                            "<goto {name}> at line {} jumps into the scope of {kind}'{lname}'",
                            g.line
                        ),
                    ));
                }
                let off = here as i64 - g.jmp_pc as i64 - 1;
                if off.unsigned_abs() > MAX_SJ as u64 {
                    return Err(self.err(g.line, "control structure too long"));
                }
                self.l().code[g.jmp_pc].set_sj(off as i32);
            } else {
                kept.push_or_abort(g);
            }
        }
        let blk = self.l().blocks.last_mut().expect("no block");
        blk.gotos = kept;
        blk.labels.push_or_abort(LabelDef {
            name,
            pc: here,
            line,
            nactive: nactive.max(first_avar),
        });
        // every defined label is a jump destination: pending gotos just got
        // patched to land at `here`, AND backward gotos resolved by
        // `goto_stat` lookup against this label will jump here too.
        self.mark_target(here);
        Ok(())
    }

    /// Compile `goto name`: backward jump if a label is visible, else a
    /// pending forward reference in the current block.
    pub(super) fn goto_stat(&mut self, name: &'a str, line: u32) -> Result<(), SyntaxError> {
        self.last_line = line;
        // PUC's `gotostat` scans only the *current* block for an
        // already-defined backward label; unresolved gotos enter the pending
        // list and percolate outward on each `leave_block`, so an inner
        // block's later `::name::` (or the enclosing block's existing one)
        // gets matched at scope exit. Searching all ancestor blocks here would
        // make `do goto l; ::l:: end` lock onto the outer `::l::` before the
        // inner one even gets defined — goto.lua 5.2/5.3 :71 specifically
        // exercises that shadow.
        let found: Option<(usize, usize)> = self
            .lr()
            .blocks
            .last()
            .and_then(|b| b.labels.iter().rev().find(|l| l.name == name))
            .map(|l| (l.pc, l.nactive));
        if let Some((pc, nactive)) = found {
            // jumping back discards locals declared after the label
            if let Some(floor) = self.reg_floor_from_avar(nactive) {
                self.emit(Inst::iabc(Op::Close, floor, 0, 0, false));
            }
            self.jump_back(pc)?;
            return Ok(());
        }
        let jmp = self.emit_jump();
        let nactive = self.lr().avars.len();
        self.l()
            .blocks
            .last_mut()
            .expect("no block")
            .gotos
            .push_or_abort(GotoRef {
                name,
                jmp_pc: jmp,
                line,
                nactive,
            });
        Ok(())
    }
}
