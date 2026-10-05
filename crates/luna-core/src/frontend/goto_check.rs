//! Parse-time `goto`/label checking, as PUC's parser does it (5.2+). PUC
//! resolves gotos while it parses, so its errors come at fixed points of
//! the parse — when a label is placed, when a block or function closes —
//! and are reported at the line the parser stands on there. (The compiler
//! resolves gotos again to generate code; by then the parse is over and
//! the position of these points is lost.)
//!
//! The three generations differ: 5.2/5.3 resolve a goto against the
//! labels of its block as soon as either appears; 5.4 resolves backward
//! gotos at the goto and forward ones when the label is placed; 5.5
//! resolves everything when the block closes. In 5.2–5.4 `break` is a
//! goto to a label every loop places at its end; 5.1 and 5.5 check it on
//! the spot.

use crate::runtime::mem::{LVec, MemRef, Oom};
use crate::version::LuaVersion;

mod records;
pub(crate) use records::VarKind;
use records::{Block, Goto, Label, Open};

/// The dialect's goto bookkeeping (PUC `Dyndata` + `BlockCnt`).
pub(crate) struct GotoCheck {
    v54: bool,
    v55: bool,
    /// where each active variable's name starts in `names`
    actvar: LVec<usize>,
    /// what each active variable is
    kinds: LVec<VarKind>,
    /// the active variables' names back to back (they are needed only for an
    /// error message, so one buffer instead of an allocation per variable)
    names: LVec<u8>,
    labels: LVec<Label>,
    pending: LVec<Goto>,
    blocks: LVec<Block>,
    /// index into `blocks` of each open function's outer block
    funcs: LVec<usize>,
    open: LVec<Open>,
}

/// Why a goto step failed: a goto or label error to report, or a memory
/// error.
pub(crate) enum GotoErr {
    Text(String),
    Mem(Oom),
}

impl From<String> for GotoErr {
    fn from(s: String) -> GotoErr {
        GotoErr::Text(s)
    }
}

impl From<Oom> for GotoErr {
    fn from(o: Oom) -> GotoErr {
        GotoErr::Mem(o)
    }
}

impl GotoCheck {
    pub(crate) fn enter_function(&mut self) {
        self.funcs.push_or_abort(self.blocks.len());
        self.enter_block(false)
    }

    pub(crate) fn enter_block(&mut self, is_loop: bool) {
        self.blocks.push_or_abort(Block {
            nactvar: self.actvar.len(),
            first_label: self.labels.len(),
            first_goto: self.pending.len(),
            is_loop,
        });
    }

    /// A variable comes into scope (a local, or a 5.5 global declaration;
    /// `global *` is named "*").
    pub(crate) fn declare(&mut self, name: &str, kind: VarKind) {
        self.actvar.push_or_abort(self.names.len());
        self.kinds.push_or_abort(kind);
        self.names.extend_from_slice_or_abort(name.as_bytes());
    }

    /// The name of active variable `i`.
    fn var_name(&self, i: usize) -> &[u8] {
        let end = self.actvar.get(i + 1).copied().unwrap_or(self.names.len());
        &self.names[self.actvar[i]..end]
    }

    fn truncate_actvar(&mut self, n: usize) {
        if let Some(&end) = self.actvar.get(n) {
            self.names.truncate(end);
            self.actvar.truncate(n);
            self.kinds.truncate(n);
        }
    }

    /// Whether `name` here is a read-only local, of this function or an
    /// enclosing one, which an assignment may not target (PUC
    /// `check_readonly`). A global declaration of the name, or a `global *`,
    /// in between leaves the answer to the compiler.
    pub(crate) fn is_const_local(&self, name: &str) -> bool {
        let name = name.as_bytes();
        for (i, &kind) in self.kinds.iter().enumerate().rev() {
            let n = self.var_name(i);
            match kind {
                VarKind::Global if n == name || n == b"*" => return false,
                VarKind::Global => {}
                _ if n == name => return kind == VarKind::Const,
                _ => {}
            }
        }
        false
    }

    fn block(&self) -> &Block {
        self.blocks.last().expect("goto block")
    }

    fn scope_error(&self, g: &Goto) -> String {
        // 5.5 drops "local": the variable may be a global declaration
        let kind = if self.v55 { "" } else { "local " };
        format!(
            "<goto {}> at line {} jumps into the scope of {kind}'{}'",
            g.name,
            g.line,
            String::from_utf8_lossy(self.var_name(g.nactvar))
        )
    }

    /// PUC `closegoto`/`solvegoto`: the goto lands on label `l`; entering
    /// the scope of a local on the way is an error.
    fn close_goto(&mut self, g: usize, l: usize) -> Result<(), GotoErr> {
        if self.pending[g].nactvar < self.labels[l].nactvar {
            return Err(self.scope_error(&self.pending[g]).into());
        }
        self.pending.remove(g);
        Ok(())
    }

    /// Resolve the pending gotos of the current block that name label `l`.
    fn find_gotos(&mut self, l: usize) -> Result<(), GotoErr> {
        let mut g = self.block().first_goto;
        while g < self.pending.len() {
            if self.pending[g].name == self.labels[l].name {
                self.close_goto(g, l)?;
            } else {
                g += 1;
            }
        }
        Ok(())
    }

    /// 5.2/5.3 `findlabel`: match goto `g` against the current block's
    /// labels.
    fn find_label_53(&mut self, g: usize) -> Result<bool, GotoErr> {
        let first = self.block().first_label;
        match (first..self.labels.len()).find(|&l| self.labels[l].name == self.pending[g].name) {
            Some(l) => self.close_goto(g, l).map(|()| true),
            None => Ok(false),
        }
    }

    /// 5.4 `findlabel`: any label visible in the current function.
    fn find_label_54(&self, name: &str) -> Option<usize> {
        let first = self.blocks[*self.funcs.last().expect("goto function")].first_label;
        (first..self.labels.len()).find(|&l| &*self.labels[l].name == name)
    }

    /// `goto name` (or `break`, as a goto to "break").
    pub(crate) fn goto_stat(&mut self, name: &str, line: u32) -> Result<(), GotoErr> {
        if self.v54 && !self.v55 && name != "break" && self.find_label_54(name).is_some() {
            return Ok(()); // backward jump, resolved on the spot
        }
        self.pending.push_or_abort(Goto {
            name: name.into(),
            line,
            nactvar: self.actvar.len(),
        });
        if !self.v54 {
            let g = self.pending.len() - 1;
            self.find_label_53(g)?;
        }
        Ok(())
    }

    /// `::name::` has been read up to its closing `::` (not consumed):
    /// 5.2/5.3 check for a repeat in the block and enter the label now.
    pub(crate) fn label_before_close(&mut self, name: &str, line: u32) -> Result<(), GotoErr> {
        let mut entry = None;
        if !self.v54 {
            let first = self.block().first_label;
            if let Some(prev) = self.labels[first..].iter().find(|l| &*l.name == name) {
                return Err(format!("label '{name}' already defined on line {}", prev.line).into());
            }
            self.labels.push_or_abort(Label {
                name: name.into(),
                line,
                nactvar: self.actvar.len(),
            });
            entry = Some(self.labels.len() - 1);
        }
        self.open.push_or_abort(Open {
            name: name.into(),
            line,
            entry,
        });
        Ok(())
    }

    pub(crate) fn has_open_labels(&self) -> bool {
        !self.open.is_empty()
    }

    /// The no-op statements after the open labels are done; `last` says
    /// the block ends here (`else`/`elseif`/`end`/<eof>), which puts the
    /// labels outside the block's locals. Labels are settled innermost
    /// first, as PUC's recursion unwinds.
    pub(crate) fn finish_labels(&mut self, last: bool) -> Result<(), GotoErr> {
        while let Some(open) = self.open.pop() {
            let nactvar = if last {
                self.block().nactvar
            } else {
                self.actvar.len()
            };
            let l = match open.entry {
                Some(l) => {
                    self.labels[l].nactvar = nactvar;
                    l
                }
                None => {
                    if let Some(prev) = self.find_label_54(&open.name) {
                        return Err(format!(
                            "label '{}' already defined on line {}",
                            open.name, self.labels[prev].line
                        )
                        .into());
                    }
                    self.labels.push_or_abort(Label {
                        name: open.name,
                        line: open.line,
                        nactvar,
                    });
                    self.labels.len() - 1
                }
            };
            if !self.v55 {
                self.find_gotos(l)?;
            }
        }
        Ok(())
    }

    /// PUC `leaveblock`: a loop places its "break" label, the block's
    /// locals and labels go out of scope, and its pending gotos move to
    /// the enclosing block — or, at a function's outer block, are errors.
    pub(crate) fn leave_block(&mut self) -> Result<(), GotoErr> {
        let blk = self.block();
        let (nactvar, first_label, first_goto, is_loop) =
            (blk.nactvar, blk.first_label, blk.first_goto, blk.is_loop);
        // 5.4 drops the block's locals before placing the break label,
        // 5.2/5.3 after (a break moved out of the body block has the same
        // level either way).
        if self.v54 && !self.v55 {
            self.truncate_actvar(nactvar);
        }
        if is_loop && !self.v55 {
            self.labels.push_or_abort(Label {
                name: "break".into(),
                line: 0,
                nactvar: self.actvar.len(),
            });
            self.find_gotos(self.labels.len() - 1)?;
        }
        if self.v55 {
            // PUC 5.5 `solvegotos`: this block's pending gotos meet its
            // labels (backward ones included) or move out a level. (PUC
            // has dropped the block's locals by now, but their names are
            // still in its array for the error message.)
            let mut g = first_goto;
            while g < self.pending.len() {
                let name = &self.pending[g].name;
                match (first_label..self.labels.len()).find(|&l| &self.labels[l].name == name) {
                    Some(l) => self.close_goto(g, l)?,
                    None => {
                        self.pending[g].nactvar = nactvar;
                        g += 1;
                    }
                }
            }
        }
        self.truncate_actvar(nactvar);
        self.labels.truncate(first_label);
        let _ = self.blocks.pop();
        if self.funcs.last() == Some(&self.blocks.len()) {
            let _ = self.funcs.pop();
            return match self.pending.get(first_goto) {
                Some(g) => Err(self.undefined(g).into()),
                None => Ok(()),
            };
        }
        let mut g = first_goto;
        while g < self.pending.len() {
            self.pending[g].nactvar = self.pending[g].nactvar.min(nactvar);
            // 5.2/5.3 retry the moved goto against the enclosing block's
            // labels; 5.4 waits for a label to be placed
            if self.v54 || !self.find_label_53(g)? {
                g += 1;
            }
        }
        Ok(())
    }

    /// PUC `undefgoto`: the first goto of a closing function that found
    /// no label.
    fn undefined(&self, g: &Goto) -> String {
        match (&*g.name, self.v54) {
            ("break", true) => format!("break outside loop at line {}", g.line),
            ("break", false) => format!("<break> at line {} not inside a loop", g.line),
            (name, _) => format!("no visible label '{name}' for <goto> at line {}", g.line),
        }
    }
}
