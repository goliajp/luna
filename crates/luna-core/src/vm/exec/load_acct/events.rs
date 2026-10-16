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
                }
            }
            PEv::LocVar => {
                if let Some(f) = r.fs.last_mut() {
                    f.loc.push(&mut self.heap, s.locvar);
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

    /// A function opens: the main one in `luaY_parser`, a nested one in
    /// `addprototype` (5.1 `open_func`), then `open_func`'s own table.
    fn replay_open(&mut self, r: &mut Replay) -> Result<(), SyntaxError> {
        let s = self.heap.sizes;
        let d = r.d;
        let main = r.opened == 0;
        if !main
            && d != Dialect::L51
            && let Some(parent) = r.fs.last_mut()
        {
            // `addprototype`: the parent's array, then the new prototype
            parent.p.push(&mut self.heap, s.ptr);
        }
        // the parser's own string of the chunk name (`luaS_new`) and the
        // scanner's buffer (`luaX_setinput`): 5.1 makes them first, the
        // others after the main prototype
        if main && d == Dialect::L51 {
            self.heap.acct_source(r.source);
            self.setinput(r)?;
        }
        // the prototype luna made without counting
        if let Some(p) = r.opening.get(r.opened) {
            let h = p.as_ptr() as *mut crate::runtime::heap::GcHeader;
            self.heap.acct_promote_obj(h, PucKind::Proto, s.proto);
        }
        if main && d != Dialect::L51 {
            self.heap.acct_source(r.source);
            self.setinput(r)?;
        }
        let h = match d {
            Dialect::L51 | Dialect::L52 | Dialect::L55 => Some(self.new_ptab(r)?),
            Dialect::L53 | Dialect::L54 => None,
        };
        if d == Dialect::L55 {
            self.anchor_in_h(r, h.as_ref().map(|t| t.id));
        }
        let ld = r.opening.get(r.opened).map_or(0, |p| p.line_defined);
        let shape = r.opened;
        r.opened += 1;
        r.fs.push(Fs {
            prev_line: ld,
            shape,
            h,
            ..Fs::default()
        });
        Ok(())
    }

    /// `luaX_setinput`: 5.2 makes the name `_ENV` (and keeps it for
    /// good), then the scanner's buffer is sized.
    fn setinput(&mut self, r: &mut Replay) -> Result<(), SyntaxError> {
        if r.d == Dialect::L52 {
            let env = self.heap.try_intern(b"_ENV")?;
            self.heap.acct_source(env);
            if !self
                .heap
                .puc
                .pins
                .iter()
                .any(|p| p.as_ptr() == env.as_ptr())
            {
                self.heap.puc.pins.push(env);
            }
        }
        r.buff = 32;
        self.heap.acct_block(0, 32);
        Ok(())
    }

    /// The innermost function closes (`close_func`): its arrays are
    /// trimmed; up to 5.2 the token read last is anchored again in the
    /// enclosing function's table, and 5.1's `pushclosure` grows the
    /// enclosing function's prototype array; 5.5 drops the function's
    /// table from the chunk's.
    fn replay_close(&mut self, r: &mut Replay, tok: i64) -> Result<(), SyntaxError> {
        let s = self.heap.sizes;
        let d = r.d;
        let line_elem = if matches!(d, Dialect::L54 | Dialect::L55) {
            1
        } else {
            4
        };
        let mut f = r.fs.pop().unwrap_or_default();
        let h = &mut self.heap;
        f.code.trim(h, 4);
        f.line.trim(h, line_elem);
        f.abs.trim(h, s.abslineinfo);
        f.k.trim(h, s.tvalue);
        f.p.trim(h, s.ptr);
        f.loc.trim(h, s.locvar);
        f.up.trim(h, s.upvaldesc);
        if let Some(p) = r.closing.get(r.closed) {
            let arrays = [
                (f.code, 4),
                (f.p, s.ptr),
                (f.k, s.tvalue),
                (f.line, line_elem),
                (f.abs, s.abslineinfo),
                (f.loc, s.locvar),
                (f.up, s.upvaldesc),
            ];
            p.puc_arrays.set(arrays.map(|(a, e)| (a.n * e) as u32));
        }
        r.closed += 1;
        if matches!(d, Dialect::L51 | Dialect::L52) && !r.fs.is_empty() {
            // `anchor_token`
            let toks = &r.scanned.strings;
            let at = toks.partition_point(|&(t, ..)| i64::from(t) < tok);
            if let Some(&(t, text, kind)) = toks.get(at)
                && i64::from(t) == tok
                && kind != ScanKind::Reserved
            {
                self.replay_scan_text(r, text, kind)?;
            }
        }
        if d == Dialect::L51
            && let Some(parent) = r.fs.last_mut()
        {
            parent.p.push(&mut self.heap, s.ptr);
        }
        if d == Dialect::L55
            && let (Some(h), Some(k)) = (r.h.as_mut(), f.h.as_ref())
        {
            h.remove(PKey::Tab(k.id));
        }
        // 5.2 on, `close_func` checks for a collection at its end
        #[cfg(feature = "gc-trace")]
        if d != Dialect::L51 {
            self.trace_checkpoint();
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
