//! The replay of what the scanner does: the strings it makes, keyed in
//! the parser's tables, and how full its buffer gets.

use super::*;

impl Replay<'_> {
    /// The table the scanner keys a string of `kind` in, if any.
    fn scan_table(&mut self, kind: ScanKind) -> Option<&mut PTab> {
        match self.d {
            Dialect::L51 | Dialect::L52 => self.fs.last_mut().and_then(|f| f.h.as_mut()),
            Dialect::L55 if kind == ScanKind::Reserved => None,
            _ => self.h.as_mut(),
        }
    }

    /// A number for that table, other than 0: the innermost function's
    /// up to 5.2, the chunk's after.
    fn table_id(&self) -> u32 {
        match self.d {
            Dialect::L51 | Dialect::L52 => self.fs.last().map_or(u32::MAX, |f| f.shape as u32 + 1),
            _ => u32::MAX,
        }
    }
}

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
        let d = r.d;
        let k = PKey::of_str(s, &mut r.long_keys);
        let val = r.scan_val(s);
        if let Some(t) = r.scan_table(kind) {
            let (new, id) = (!t.contains(k), t.id);
            self.set_str_in(t, d, s, k)?;
            if new {
                r.note(id, PVal::V(Value::Str(s)), Some(val));
                self.scan_checkpoint(r, s)?;
            }
        }
        Ok(())
    }

    /// `luaX_newstring` checks for a collection when it anchors a new
    /// string `s` (5.5 `anchorstr`), which it holds on the stack meanwhile
    /// (from 5.2).
    fn scan_checkpoint(
        &mut self,
        r: &mut Replay,
        s: Gc<crate::runtime::LuaStr>,
    ) -> Result<(), SyntaxError> {
        let temp = (r.d != Dialect::L51).then_some(s);
        self.load_checkpoint(r, temp)
    }

    /// [`Vm::replay_scan`] of a recorded token: a name or a literal is
    /// made once, and put once in each table.
    pub(super) fn replay_scan_text(
        &mut self,
        r: &mut Replay,
        text: ScanText,
        kind: ScanKind,
    ) -> Result<(), SyntaxError> {
        let (names, src) = (r.names, r.src);
        let slot = match text {
            ScanText::Sym(sym) => &mut r.syms[sym.0 as usize],
            ScanText::Reserved(k, _) => &mut r.reserved[usize::from(k)],
        };
        let s = match slot.0 {
            Some(s) => s,
            None => {
                let bytes = text.bytes(names, src);
                let s = self.heap.try_intern(bytes)?;
                slot.0 = Some(s);
                if bytes.len() > crate::runtime::string::MAX_SHORT_LEN && r.d != Dialect::L51 {
                    r.longs.push(s);
                }
                s
            }
        };
        // a table's keys stay until the load is done
        let id = r.table_id();
        let in_table = match text {
            ScanText::Sym(sym) => r.syms[sym.0 as usize].1,
            ScanText::Reserved(k, _) => r.reserved[usize::from(k)].1,
        };
        let d = r.d;
        let k = PKey::of_str(s, &mut r.long_keys);
        let val = r.scan_val(s);
        let Some(t) = r.scan_table(kind).filter(|_| in_table != id) else {
            return Ok(());
        };
        let (new, tab) = (!t.contains(k), t.id);
        self.set_str_in(t, d, s, k)?;
        match text {
            ScanText::Sym(sym) => r.syms[sym.0 as usize].1 = id,
            ScanText::Reserved(k, _) => r.reserved[usize::from(k)].1 = id,
        }
        if new {
            r.note(tab, PVal::V(Value::Str(s)), Some(val));
            self.scan_checkpoint(r, s)?;
        }
        Ok(())
    }

    /// The log's events, with what the scanner does up to each one's token:
    /// it fills its buffer while it reads a token, then makes the token's
    /// string. A load that failed stops at `cut`.
    pub(super) fn replay_log(
        &mut self,
        r: &mut Replay,
        log: &[(TokRef, PEv)],
        chunk: &crate::frontend::ast::Chunk,
        marks: &crate::frontend::parser::Marks,
        cut: Option<Cut>,
    ) -> Result<(), SyntaxError> {
        let scanned = r.scanned;
        let mut strs = scanned.strings.iter().peekable();
        let mut peaks = scanned.peaks.iter().peekable();
        // the token of the event at `i`: one logged at `TokRef::Next`
        // takes the token of the next event that names one
        let at = |t: TokRef| match t {
            TokRef::Start => -1,
            t => i64::from(t.resolve(chunk, marks)),
        };
        let mut next: Option<(usize, i64)> = None;
        // the token the previous event named, and the first token at
        // which the scanner has something left to do
        let mut last = (TokRef::Start, -1);
        let mut due = i64::MIN;
        for (i, &(t, ev)) in log.iter().enumerate() {
            let tok = if t == last.0 {
                last.1
            } else if t == TokRef::Next {
                match next {
                    Some((j, tok)) if j > i => tok,
                    _ => {
                        let j = log[i..]
                            .iter()
                            .position(|&(t, _)| t != TokRef::Next)
                            .map(|n| i + n);
                        let tok = j.map_or(i64::from(marks.eof), |j| at(log[j].0));
                        next = Some((j.unwrap_or(log.len()), tok));
                        tok
                    }
                }
            } else {
                let tok = at(t);
                last = (t, tok);
                tok
            };
            // an event at `Next` is done with the one it is made for
            let made_at = match (t, next) {
                (TokRef::Next, Some((j, _))) if j < log.len() => log[j].0,
                _ => t,
            };
            if let Some(c) = cut
                && tok >= c.events
                && !(c.ended && tok == c.events && c.takes(made_at, ev, marks))
            {
                break;
            }
            if crate::runtime::gc_hooks::TRACE {
                self.heap.trace_line(format_args!("E {tok} {ev:?}"));
            }
            if tok >= due {
                self.scan_until(r, &mut strs, &mut peaks, tok)?;
                due = [
                    peaks.peek().map(|&&(t, _)| i64::from(t)),
                    strs.peek().map(|(t, ..)| i64::from(*t)),
                    r.splices.front().map(|&(t, _)| t),
                ]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(i64::MAX);
            }
            self.replay_event(r, tok, ev)?;
        }
        if let Some(c) = cut {
            self.scan_until(r, &mut strs, &mut peaks, c.scans)?;
        }
        // the reader's last calls, past the last token
        while let Some((_, d)) = r.splices.pop_front() {
            self.heap.replay_reader_call(d);
        }
        Ok(())
    }

    /// What the scanner does up to token `tok`.
    fn scan_until<'a>(
        &mut self,
        r: &mut Replay,
        strs: &mut std::iter::Peekable<impl Iterator<Item = &'a (u32, ScanText, ScanKind)>>,
        peaks: &mut std::iter::Peekable<impl Iterator<Item = &'a (u32, u32)>>,
        tok: i64,
    ) -> Result<(), SyntaxError> {
        loop {
            let p = peaks.peek().map(|&&(t, _)| i64::from(t));
            let st = strs.peek().map(|(t, ..)| i64::from(*t));
            // a reader call while the token is read: before what the scan
            // of the token makes
            if let Some(&(s, _)) = r.splices.front()
                && s <= tok
                && p.is_none_or(|p| s <= p)
                && st.is_none_or(|st| s <= st)
            {
                let (_, d) = r.splices.pop_front().expect("peeked");
                self.heap.replay_reader_call(d);
                continue;
            }
            match (p, st) {
                (Some(p), _) if p <= tok && st.is_none_or(|st| p <= st) => {
                    let &(_, peak) = peaks.next().expect("peeked");
                    self.grow_buff(r, peak as usize);
                }
                (_, Some(st)) if st <= tok => {
                    let &(_, text, kind) = strs.next().expect("peeked");
                    self.replay_scan_text(r, text, kind)?;
                }
                _ => return Ok(()),
            }
        }
    }

    /// The scanner's buffer holding `n` bytes: each save that overflows
    /// grows it once.
    pub(super) fn grow_buff(&mut self, r: &mut Replay, n: usize) {
        while r.buff < n {
            let size = crate::frontend::parser::scan::grown(self.version, r.buff, r.buff + 1);
            self.heap.acct_block(r.buff, size);
            r.buff = size;
        }
    }
}
