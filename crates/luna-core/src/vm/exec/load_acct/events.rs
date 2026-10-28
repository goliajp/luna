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
                self.replay_code(r, tok, line, n);
                Ok(())
            }
            ev => self.replay_other(r, tok, ev),
        }
    }

    /// `n` instructions luna emitted on `line` at token `tok`: the PUC
    /// instructions that stand for each, with their lines, and the
    /// constants PUC adds for it (an instruction PUC has none for stands
    /// for none).
    fn replay_code(&mut self, r: &mut Replay, tok: i64, line: u32, n: u32) {
        if r.far {
            return self.replay_code_far(r, tok, line, n);
        }
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

    /// [`Self::replay_code`] in a chunk whose lines count (see
    /// [`Replay::far`]): each PUC instruction coded on the line the last
    /// token read ended on, the ones PUC moves then moved to their own line.
    #[inline(never)]
    fn replay_code_far(&mut self, r: &mut Replay, tok: i64, line: u32, n: u32) {
        let emit = r.lastline(tok);
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
                h.parser_code(a, emit);
                continue;
            };
            let mut left = 0;
            let added = lay.code(p, pc, line, |_| left += 1);
            let moved = lay.moved(p, pc).min(left);
            lay.code(p, pc, line, |l| {
                h.parser_code(a, emit);
                left -= 1;
                if left < moved && l != emit {
                    h.parser_fix(a, l);
                }
            });
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
                // `setvararg` codes it where it is: no line moves it
                let emit = r.emit_line(tok);
                let Some(f) = r.fs.last_mut() else {
                    return Ok(());
                };
                if let Some(p) = r.opening.get(f.shape) {
                    let Fs { a, lay, .. } = f;
                    lay.prologue(p, |l| self.heap.parser_code(a, emit.unwrap_or(l)));
                }
            }
            PEv::Lit(i) => {
                let known = r.strs.and_then(|t| t.lits[usize::from(i)]);
                let text = crate::compiler::LITERALS[usize::from(i)];
                self.replay_scan(r, text.as_bytes(), known)?
            }
            PEv::Close => self.replay_close(r, tok)?,
            PEv::Grow(line, n) => self.replay_code(r, tok, line, n),
            PEv::CodeUp(line) => {
                let child = r.fs.pop();
                self.replay_code(r, tok, line, 1);
                r.fs.extend(child);
            }
            PEv::Use => {}
            PEv::Fix(line) => {
                if let Some(f) = r.fs.last_mut() {
                    self.heap.parser_fix(&mut f.a, line);
                }
            }
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

impl Replay<'_> {
    /// From 5.4: the line the last token read ended on while the scanner
    /// is on token `tok`. Instructions come in the order of their tokens,
    /// but for a closure's, which a function closing after it takes back.
    #[inline]
    pub(super) fn lastline(&mut self, tok: i64) -> u32 {
        let lines = &self.scanned.lines;
        let mut i = self.line_at;
        if i > 0 && tok < i64::from(lines[i - 1].0) {
            i = lines.partition_point(|&(t, _)| i64::from(t) <= tok);
        }
        while i < lines.len() && i64::from(lines[i].0) <= tok {
            i += 1;
        }
        self.line_at = i;
        i.checked_sub(1).map_or(1, |j| lines[j].1)
    }

    /// The line an instruction made at token `tok` is coded on, when it
    /// counts ([`Self::far`]): the last token's (see [`Self::lastline`]).
    #[inline]
    pub(super) fn emit_line(&mut self, tok: i64) -> Option<u32> {
        if self.far {
            Some(self.lastline(tok))
        } else {
            None
        }
    }
}
