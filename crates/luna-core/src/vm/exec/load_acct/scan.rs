//! The replay of what the scanner does: the strings it makes, keyed in
//! the parser's tables, and how full its buffer gets.

use super::*;

impl Replay<'_> {
    /// The table the scanner keys a string of `kind` in, if any.
    fn scan_table(&self, kind: ScanKind) -> Option<Gc<Table>> {
        match self.d {
            Dialect::L51 | Dialect::L52 => self.fs.last().and_then(|f| f.h),
            Dialect::L55 if kind == ScanKind::Reserved => None,
            _ => self.h,
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
        match r.scan_table(kind) {
            Some(t) => {
                #[cfg(feature = "gc-trace")]
                let new = t.get(Value::Str(s)).is_nil();
                self.set_in(t, Value::Str(s), Value::Bool(true))?;
                #[cfg(feature = "gc-trace")]
                self.scan_checkpoint(new);
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// `luaX_newstring` checks for a collection when it anchors a new
    /// string (5.5 `anchorstr`): the accounting trace marks where (the
    /// collection itself waits for the load).
    #[cfg(feature = "gc-trace")]
    fn scan_checkpoint(&mut self, new: bool) {
        if new {
            self.trace_checkpoint();
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
        let Some(t) = r.scan_table(kind) else {
            return Ok(());
        };
        // a table's keys stay until the load is done
        let id = r.table_id();
        let slot = match text {
            ScanText::Sym(sym) => &mut r.syms[sym.0 as usize].1,
            ScanText::Reserved(k, _) => &mut r.reserved[usize::from(k)].1,
        };
        if *slot != id {
            *slot = id;
            #[cfg(feature = "gc-trace")]
            let new = t.get(Value::Str(s)).is_nil();
            self.set_in(t, Value::Str(s), Value::Bool(true))?;
            #[cfg(feature = "gc-trace")]
            self.scan_checkpoint(new);
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
        for (i, &(t, ev)) in log.iter().enumerate() {
            let tok = if t == TokRef::Next {
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
                at(t)
            };
            if let Some(c) = cut
                && tok >= c.events
                && !(c.ended
                    && tok == c.events
                    && matches!(t, TokRef::After(_, 0) | TokRef::StatAfter(_, 0)))
            {
                break;
            }
            #[cfg(feature = "gc-trace")]
            self.heap.trace.line(format_args!("E {tok} {ev:?}"));
            self.scan_until(r, &mut strs, &mut peaks, tok)?;
            self.replay_event(r, tok, ev)?;
        }
        if let Some(c) = cut {
            self.scan_until(r, &mut strs, &mut peaks, c.scans)?;
        }
        // the reader's last calls, past the last token
        while let Some((_, d)) = r.splices.pop_front() {
            self.heap.defer_apply(d);
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
                self.heap.defer_apply(d);
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
