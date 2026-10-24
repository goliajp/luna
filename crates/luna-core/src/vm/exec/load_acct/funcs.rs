//! The replay of a function opening and closing: its prototype, its
//! table, and the arrays trimmed when it ends.

use super::*;
use crate::runtime::puc::records::parse::set_parsing;

impl Vm {
    /// A function opens: the main one in `luaY_parser`, a nested one in
    /// `addprototype` (5.1 `open_func`), then `open_func`'s own table.
    pub(super) fn replay_open(&mut self, r: &mut Replay) -> Result<(), SyntaxError> {
        let s = self.heap.puc_sizes();
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
        if let Some(&p) = r.opening.get(r.opened) {
            let h = p.hdr();
            self.heap.acct_promote_obj(h, PucKind::Proto, s.proto);
            if r.rec.is_none() {
                set_parsing(p, true);
                if !main && d != Dialect::L51 {
                    self.proto_barrier(r, r.fs.len() - 1, h);
                }
            }
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
            self.anchor_in_h(r, h.as_ref().map(|t| (t.id, t.tab_hdr())))?;
        }
        let ld = r.opening.get(r.opened).map_or(0, |p| p.line_defined);
        let shape = r.opened;
        r.opened += 1;
        r.fs.push(Fs {
            prev_line: ld,
            shape,
            h,
            kbase: r.kstr.len(),
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
            self.heap.pin(env);
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
    pub(super) fn replay_close(&mut self, r: &mut Replay, tok: i64) -> Result<(), SyntaxError> {
        let s = self.heap.puc_sizes();
        let d = r.d;
        let line_elem = if matches!(d, Dialect::L54 | Dialect::L55) {
            1
        } else {
            4
        };
        let mut f = r.fs.pop().unwrap_or_default();
        r.kstr.truncate(f.kbase);
        let closed = r.opening.get(f.shape).copied();
        if let Some(p) = closed {
            set_parsing(p, false);
            self.heap.settle_parsed(p);
        }
        let h = &mut self.heap;
        f.code.trim(h, 4);
        f.line.trim(h, line_elem);
        f.abs.trim(h, s.abslineinfo);
        f.k.trim(h, s.tvalue);
        f.p.trim(h, s.ptr);
        f.loc.trim(h, s.locvar);
        f.up.trim(h, s.upvaldesc);
        if let Some(p) = r.closing.get(r.closed) {
            // trimmed: each array's size is its count
            p.puc_arrays.set(f.puc_caps(d, &s));
        }
        r.closed += 1;
        if matches!(d, Dialect::L51 | Dialect::L52) && !r.fs.is_empty() {
            // `anchor_token`; 5.2's function table is on the stack still
            let toks = &r.scanned.strings;
            let at = toks.partition_point(|&(t, ..)| i64::from(t) < tok);
            if let Some(&(t, text, kind)) = toks.get(at)
                && i64::from(t) == tok
                && kind != ScanKind::Reserved
            {
                r.closing_tab =
                    f.h.as_ref()
                        .and_then(PTab::tab)
                        .filter(|_| d == Dialect::L52);
                let anchored = self.replay_scan_text(r, text, kind);
                r.closing_tab = None;
                anchored?;
            }
        }
        if d == Dialect::L51
            && let Some(parent) = r.fs.last_mut()
        {
            parent.p.push(&mut self.heap, s.ptr);
            // `pushclosure`
            if let Some(p) = closed {
                self.proto_barrier(r, r.fs.len() - 1, p.hdr());
            }
        }
        if d == Dialect::L55
            && let (Some(h), Some(k)) = (r.h.as_mut(), f.h.as_ref())
        {
            let tab = k.tab().unwrap_or(std::ptr::null_mut());
            h.remove(&mut self.heap, PKey::Tab(k.id), tab);
            let (h, k) = (h.id, k.id);
            r.note(h, PVal::Tab(k), None);
        }
        // 5.2 on, `close_func` checks for a collection at its end
        if d != Dialect::L51 {
            self.load_checkpoint(r, None)?;
        }
        Ok(())
    }
}
