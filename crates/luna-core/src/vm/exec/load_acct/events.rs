//! The replay of one scanner string or one parser event.

use super::*;
use crate::runtime::puc::api::PucParse;

impl Vm {
    /// One event of the log, logged at token `tok`.
    #[inline]
    pub(super) fn replay_event(
        &mut self,
        r: &mut Replay,
        tok: i64,
        ev: PEv,
    ) -> Result<(), SyntaxError> {
        match ev {
            PEv::Grow(line, n) => {
                self.replay_code(r, line, n);
                Ok(())
            }
            ev => self.replay_other(r, tok, ev),
        }
    }

    /// `n` instructions luna emitted on `line`: the PUC instructions that
    /// stand for each, with their lines, and the constants PUC adds for it
    /// (an instruction PUC has none for stands for none).
    fn replay_code(&mut self, r: &mut Replay, line: u32, n: u32) {
        let Replay { fs, opening, .. } = r;
        let Some(f) = fs.last_mut() else {
            return;
        };
        let h = &mut self.heap;
        let from = f.pc;
        f.pc += n as usize;
        let p = opening.get(f.shape);
        let Fs { a, lay, .. } = f;
        for pc in from..from + n as usize {
            let Some(p) = p.filter(|p| pc < p.code.len()) else {
                h.parser_code(a, line);
                continue;
            };
            let added = lay.code(p, pc, line, |l| h.parser_code(a, l));
            for _ in 0..added {
                h.parser_push(&mut a.k, ParserArray::Consts);
            }
        }
    }

    /// An event other than an instruction.
    fn replay_other(&mut self, r: &mut Replay, tok: i64, ev: PEv) -> Result<(), SyntaxError> {
        match ev {
            PEv::Open => self.replay_open(r)?,
            PEv::Prologue => {
                // a vararg function's first instructions (`setvararg`)
                let Some(f) = r.fs.last_mut() else {
                    return Ok(());
                };
                if let Some(p) = r.opening.get(f.shape) {
                    let Fs { a, lay, .. } = f;
                    lay.prologue(p, |l| self.heap.parser_code(a, l));
                }
            }
            PEv::Lit(i) => {
                let known = r.strs.and_then(|t| t.lits[usize::from(i)]);
                let text = crate::compiler::LITERALS[usize::from(i)];
                self.replay_scan(r, text.as_bytes(), known)?
            }
            PEv::Close => self.replay_close(r, tok)?,
            PEv::Grow(line, n) => self.replay_code(r, line, n),
            PEv::CodeUp(line) => {
                let child = r.fs.pop();
                self.replay_code(r, line, 1);
                r.fs.extend(child);
            }
            PEv::Use => {}
            PEv::Uncode => {
                if let Some(f) = r.fs.last_mut() {
                    f.pc -= 1;
                    self.heap.parser_uncode(&mut f.a);
                }
            }
            PEv::Const(key, v) => self.replay_const(r, key, v)?,
            PEv::Upval(depth) => {
                if let Some(f) = r.fs.get_mut(depth as usize) {
                    self.heap.parser_push(&mut f.a.up, ParserArray::Upvals);
                    let n = f.a.up.n;
                    self.name_barrier(r, depth as usize, n, true);
                }
            }
            PEv::LocVar => {
                if let Some(f) = r.fs.last_mut() {
                    self.heap.parser_push(&mut f.a.loc, ParserArray::LocVars);
                    let n = f.a.loc.n;
                    self.name_barrier(r, r.fs.len() - 1, n, false);
                }
            }
            // 5.1 keeps its active variables in the function, with no
            // labels: nothing grows
            PEv::ActVar(_) | PEv::Goto(_) | PEv::Label(_) if r.d == Dialect::L51 => {}
            PEv::ActVar(n) => {
                // `new_localvar` asks for room past the new entry
                self.heap
                    .parser_reach(&mut r.actvar, ParserArray::ActVar, n as usize + 1);
                r.actvar.n = n as usize;
            }
            PEv::Goto(n) => {
                self.heap
                    .parser_reach(&mut r.gt, ParserArray::Gotos, n as usize);
            }
            PEv::Label(n) => {
                self.heap
                    .parser_reach(&mut r.label, ParserArray::Labels, n as usize);
            }
        }
        Ok(())
    }
}
