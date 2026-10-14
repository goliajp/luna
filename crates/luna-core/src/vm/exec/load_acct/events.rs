//! The replay of one scanner string or one parser event.

use super::*;

impl Vm {
    /// The scanner makes the string of a token (`luaX_newstring`): the
    /// string, then the entry that anchors it, in the function's table up
    /// to 5.2, in the chunk's from 5.3 (5.5 does not anchor reserved
    /// words).
    pub(super) fn replay_scan(
        &mut self,
        r: &mut Replay,
        bytes: &[u8],
        kind: ScanKind,
    ) -> Result<(), SyntaxError> {
        let s = self.heap.try_intern(bytes)?;
        match r.scan_table(kind) {
            Some(t) => self.set_in(t, Value::Str(s), Value::Bool(true)),
            None => Ok(()),
        }
    }

    /// [`Vm::replay_scan`] of a recorded token: a name or a literal is
    /// made once, and put once in each table.
    pub(super) fn replay_scan_text(
        &mut self,
        r: &mut Replay,
        text: ScanText,
        kind: ScanKind,
    ) -> Result<(), SyntaxError> {
        let ScanText::Sym(sym) = text else {
            return self.replay_scan(r, text.bytes(r.names, r.src), kind);
        };
        let i = sym.0 as usize;
        let s = match r.syms[i].0 {
            Some(s) => s,
            None => {
                let s = self.heap.try_intern(r.names.bytes(sym))?;
                r.syms[i].0 = Some(s);
                s
            }
        };
        let Some(t) = r.scan_table(kind) else {
            return Ok(());
        };
        // a table's keys stay until the load is done
        let id = r.table_id();
        if r.syms[i].1 != id {
            self.set_in(t, Value::Str(s), Value::Bool(true))?;
            r.syms[i].1 = id;
        }
        Ok(())
    }

    /// One event of the log, logged at token `tok`.
    pub(super) fn replay_event(
        &mut self,
        r: &mut Replay,
        tok: i64,
        ev: PEv,
    ) -> Result<(), SyntaxError> {
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
            PEv::Code(line) => {
                let Some(f) = r.fs.last() else {
                    return Ok(());
                };
                let (shape, pc) = (f.shape, f.pc);
                // the PUC instructions that stand for this one, with their
                // lines, and the constants PUC adds for it
                // (an instruction PUC has none for stands for none)
                let (from, to) = r.shapes.get(shape).map_or((0, 1), |sh| {
                    let a = sh.pc_map.get(pc).copied().unwrap_or(0) as usize;
                    let b = sh.pc_map.get(pc + 1).copied().unwrap_or(a as u32 + 1) as usize;
                    (a, b.max(a))
                });
                for j in from..to {
                    let l = r
                        .shapes
                        .get(shape)
                        .and_then(|sh| sh.lines.get(j).copied())
                        .unwrap_or(line);
                    self.replay_code(r, l);
                }
                let added = r
                    .shapes
                    .get(shape)
                    .map_or(0, |sh| sh.k_added.iter().filter(|&&p| p == pc).count());
                for _ in 0..added {
                    if let Some(f) = r.fs.last_mut() {
                        f.k.push(&mut self.heap, s.tvalue);
                    }
                }
                if let Some(f) = r.fs.last_mut() {
                    f.pc += 1;
                }
            }
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
            Dialect::L51 | Dialect::L52 | Dialect::L55 => Some(self.heap.try_new_table()?),
            Dialect::L53 | Dialect::L54 => None,
        };
        if d == Dialect::L55 {
            self.anchor_in_h(r, h)?;
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
            && let (Some(h), Some(k)) = (r.h, f.h)
        {
            self.store_in(h, Value::Table(k), Value::Nil)?;
        }
        Ok(())
    }

    /// One PUC instruction on `line` (`luaK_code`): the code array, then
    /// the line info (5.4+ `savelineinfo`, with its absolute entries).
    pub(super) fn replay_code(&mut self, r: &mut Replay, line: u32) {
        let d = r.d;
        let s = self.heap.sizes;
        let Some(f) = r.fs.last_mut() else {
            return;
        };
        let h = &mut self.heap;
        f.code.push(h, 4);
        if matches!(d, Dialect::L54 | Dialect::L55) {
            let dif = i64::from(line) - i64::from(f.prev_line);
            let wthabs = f.iwthabs;
            f.iwthabs += 1;
            f.last_prev = f.prev_line;
            f.last_abs = dif.abs() >= 0x80 || wthabs >= 128;
            if f.last_abs {
                f.abs.push(h, s.abslineinfo);
                f.iwthabs = 1;
            }
            f.prev_line = line;
            f.line.push(h, 1);
        } else {
            f.line.push(h, 4);
        }
    }
}
