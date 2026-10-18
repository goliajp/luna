//! The replay of one scanner string or one parser event.

use super::*;

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
            PEv::Code(line) => {
                self.replay_instruction(r, line);
                Ok(())
            }
            ev => self.replay_other(r, tok, ev),
        }
    }

    /// An instruction luna emitted on `line`: the PUC instructions that
    /// stand for it, with their lines, and the constants PUC adds for it
    /// (an instruction PUC has none for stands for none).
    fn replay_instruction(&mut self, r: &mut Replay, line: u32) {
        let (d, sizes) = (r.d, &self.heap.sizes);
        let (abs, tvalue) = (sizes.abslineinfo, sizes.tvalue);
        let Replay { fs, shapes, .. } = r;
        let Some(f) = fs.last_mut() else {
            return;
        };
        let h = &mut self.heap;
        let pc = f.pc;
        f.pc += 1;
        let Some(sh) = shapes.get(f.shape) else {
            code_one(f, h, d, abs, line);
            return;
        };
        let a = sh.pc_map.get(pc).copied().unwrap_or(0) as usize;
        let b = sh.pc_map.get(pc + 1).copied().unwrap_or(a as u32 + 1) as usize;
        for j in a..b.max(a) {
            code_one(f, h, d, abs, sh.lines.get(j).copied().unwrap_or(line));
        }
        for _ in sh.k_added.iter().filter(|&&p| p == pc) {
            f.k.push(h, tvalue);
        }
    }

    /// An event other than an instruction.
    fn replay_other(&mut self, r: &mut Replay, tok: i64, ev: PEv) -> Result<(), SyntaxError> {
        let s = self.heap.sizes;
        match ev {
            PEv::Open => self.replay_open(r)?,
            PEv::Prologue => {
                // a vararg function's first instructions (`setvararg`)
                let Some(f) = r.fs.last() else {
                    return Ok(());
                };
                let (shape, ld) = (f.shape, f.prev_line);
                let pro = r
                    .shapes
                    .get(shape)
                    .map_or(0, |sh| sh.pc_map.first().copied().unwrap_or(0));
                for j in 0..pro as usize {
                    let line = r.shapes[shape].lines.get(j).copied().unwrap_or(ld);
                    self.replay_code(r, line);
                }
            }
            PEv::Lit(text) => self.replay_scan(r, text.as_bytes(), ScanKind::Str)?,
            PEv::Close => self.replay_close(r, tok)?,
            PEv::Code(line) => self.replay_instruction(r, line),
            PEv::CodeUp(line) => {
                let child = r.fs.pop();
                let coded = self.replay_event(r, tok, PEv::Code(line));
                r.fs.extend(child);
                coded?;
            }
            PEv::Use => {}
            PEv::Uncode => {
                if let Some(f) = r.fs.last_mut() {
                    f.pc -= 1;
                    f.code.n -= 1;
                    f.line.n -= 1;
                    // 5.4+ `removelastlineinfo`
                    if f.last_abs {
                        f.abs.n -= 1;
                        f.iwthabs = 129;
                    } else {
                        f.prev_line = f.last_prev;
                        f.iwthabs = f.iwthabs.saturating_sub(1);
                    }
                }
            }
            PEv::Const(key, v) => self.replay_const(r, key, v)?,
            PEv::Upval(depth) => {
                if let Some(f) = r.fs.get_mut(depth as usize) {
                    f.up.push(&mut self.heap, s.upvaldesc);
                    let n = f.up.n;
                    self.name_barrier(r, depth as usize, n, true);
                }
            }
            PEv::LocVar => {
                if let Some(f) = r.fs.last_mut() {
                    f.loc.push(&mut self.heap, s.locvar);
                    let n = f.loc.n;
                    self.name_barrier(r, r.fs.len() - 1, n, false);
                }
            }
            // 5.1 keeps its active variables in the function, with no
            // labels: nothing grows
            PEv::ActVar(_) | PEv::Goto(_) | PEv::Label(_) if r.d == Dialect::L51 => {}
            PEv::ActVar(n) => {
                // `new_localvar` asks for room past the new entry
                let (vd, _) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.actvar.reach(&mut self.heap, n as usize + 1, vd);
                r.actvar.n = n as usize;
            }
            PEv::Goto(n) => {
                let (_, ld) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.gt.reach(&mut self.heap, n as usize, ld);
            }
            PEv::Label(n) => {
                let (_, ld) = crate::runtime::puc_size::dyd_sizes(r.d);
                r.label.reach(&mut self.heap, n as usize, ld);
            }
        }
        Ok(())
    }

    /// One PUC instruction on `line` (`luaK_code`): the code array, then
    /// the line info (5.4+ `savelineinfo`, with its absolute entries).
    pub(super) fn replay_code(&mut self, r: &mut Replay, line: u32) {
        let abs = self.heap.sizes.abslineinfo;
        if let Some(f) = r.fs.last_mut() {
            code_one(f, &mut self.heap, r.d, abs, line);
        }
    }
}

/// One PUC instruction on `line` in function `f`: the code array, then the
/// line info (5.4+ `savelineinfo`, with its absolute entries of `abs`
/// bytes).
#[inline(always)]
fn code_one(f: &mut Fs, h: &mut Heap, d: Dialect, abs: usize, line: u32) {
    f.code.push(h, 4);
    if matches!(d, Dialect::L54 | Dialect::L55) {
        let dif = i64::from(line) - i64::from(f.prev_line);
        let wthabs = f.iwthabs;
        f.iwthabs += 1;
        f.last_prev = f.prev_line;
        f.last_abs = dif.abs() >= 0x80 || wthabs >= 128;
        if f.last_abs {
            f.abs.push(h, abs);
            f.iwthabs = 1;
        }
        f.prev_line = line;
        f.line.push(h, 1);
    } else {
        f.line.push(h, 4);
    }
}
