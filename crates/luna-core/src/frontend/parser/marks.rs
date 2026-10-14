//! Token positions recorded by a parse, to place a compile error and to
//! count a load as PUC allocates it.
//!
//! PUC generates code while it parses, so an error its code generator
//! raises (a jump or a register past its limit) shows the token the
//! scanner is on and is reported at that token's line. luna compiles the
//! finished tree; the compiler names that token in terms of the tree (a
//! [`crate::compiler::TokRef`]), and a load that fails that way parses the
//! source again, recording where the nodes start and end, to find it. A
//! load that is counted records the same while it parses, with the strings
//! PUC's scanner makes.

use super::*;

/// Where the nodes of a parse are among its tokens (the first token is
/// number 0): what [`crate::compiler::TokRef`] is resolved against.
#[derive(Default)]
pub(crate) struct Marks {
    /// by `ExprId`: the token after the expression
    pub(crate) expr_follow: Vec<u32>,
    /// `(ExprId, token)` of each table constructor's `{` and function
    /// literal's `(`, by `ExprId`
    pub(crate) expr_open: Vec<(u32, u32)>,
    /// by `StatId`: the statement's first token
    pub(crate) stat_first: Vec<u32>,
    /// by `StatId`: the token after the statement
    pub(crate) stat_follow: Vec<u32>,
    /// by `StatId`: a range of `marks`, the tokens a statement's own
    /// grammar marks (see [`Parser::mark`])
    pub(crate) stat_marks: Vec<(u32, u32)>,
    pub(crate) marks: Vec<u32>,
    /// the `<eof>` token
    pub(crate) eof: u32,
    /// the statements being read: their first token and where their marks
    /// start in `pending`
    open: Vec<(u32, usize)>,
    pending: Vec<u32>,
}

/// A recording parse: the marks, or the line and near-token text of one
/// token.
#[derive(Default)]
pub(crate) struct Recording {
    pub(crate) marks: Marks,
    /// with `strings`: each token PUC's scanner makes a string for (a
    /// name, a reserved word, a string literal), and the tokens for which
    /// the scanner's buffer holds more than its first 32 bytes, with how
    /// many; and where the last token read ended
    strings: bool,
    pub(crate) scanned: Scanned,
    last_end: usize,
    /// from 5.3: the names numbered below this one, and the reserved words
    /// whose bits are set, were recorded already
    next_sym: u32,
    reserved_seen: u32,
    /// the token to report, and what was found for it
    target: Option<u32>,
    found: Option<Found>,
}

/// A token of the source: the line PUC reports an error on it at, the
/// token as `near` shows it, and where it is in the source.
pub(crate) struct Found {
    pub(crate) line: u32,
    pub(crate) near: Vec<u8>,
    pub(crate) span: Span,
}

impl Marks {
    /// The marks statement `sid` recorded.
    pub(crate) fn stat_marks(&self, sid: StatId) -> &[u32] {
        let (start, len) = self.stat_marks[sid.0 as usize];
        &self.marks[start as usize..(start + len) as usize]
    }

    /// The `{` or `(` token expression `eid` opened with.
    pub(crate) fn expr_open(&self, eid: ExprId) -> Option<u32> {
        self.expr_open
            .binary_search_by_key(&eid.0, |&(e, _)| e)
            .ok()
            .map(|i| self.expr_open[i].1)
    }
}

/// Parse `src` again, as a load at `c_depth` did, recording [`Marks`].
pub(crate) fn record_marks(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
) -> Result<Marks, SyntaxError> {
    record(src, version, c_depth, None).map(|r| r.marks)
}

/// Token `ix` of `src` as PUC's scanner, on it, reports an error at. A
/// source that fails to parse has the tokens up to where it fails.
pub(crate) fn token_at(src: &[u8], version: LuaVersion, c_depth: u32, ix: u32) -> Option<Found> {
    record(src, version, c_depth, Some(ix)).ok()?.found
}

#[path = "scan_buf.rs"]
mod scan_buf;
use scan_buf::{ScanKind, ScanText, Scanned};
/// What the load's replay needs of the scanner's record.
pub(crate) mod scan {
    pub(crate) use super::scan_buf::grown;
    pub(crate) use super::scan_buf::{RESERVED, ScanKind, ScanText, Scanned};
}

fn record(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    target: Option<u32>,
) -> Result<Recording, SyntaxError> {
    // held past the vectors made through it (see `parse_at_depth`)
    let mem = MemOwner::system();
    let mut scratch = ParseScratch::new(mem.clone());
    let names = scratch.chunk.names.take();
    let lex = Lexer::interning(src, version, names, scratch.lex_buf.take());
    let mut lex = TokenSource::Lexer(lex);
    let cur = lex.next_token()?;
    let ParseScratch {
        chunk,
        end_lines,
        stacks,
        ..
    } = &mut scratch;
    let mut p = Parser::new(lex, cur, version, c_depth, chunk, end_lines, stacks)?;
    p.start_recording(Some(Box::new(Recording {
        target,
        ..Recording::default()
    })));
    let r = p.main(src.len());
    let rec = p.stop_recording().expect("recording");
    if let Err(e) = r
        && rec.found.is_none()
    {
        return Err(e);
    }
    drop(p);
    drop(scratch);
    drop(mem);
    Ok(*rec)
}

impl Marks {
    /// Emptied for the next parse, keeping its vectors.
    fn clear(&mut self) {
        self.expr_follow.clear();
        self.expr_open.clear();
        self.stat_first.clear();
        self.stat_follow.clear();
        self.stat_marks.clear();
        self.marks.clear();
        self.eof = 0;
        self.open.clear();
        self.pending.clear();
    }
}

impl Parser<'_> {
    /// Record this parse in `rec` (emptied first, keeping its vectors), or
    /// a new recording, with the strings the scanner makes.
    pub(super) fn record_load(&mut self, rec: Option<Box<Recording>>) {
        let mut rec = rec.unwrap_or_default();
        rec.marks.clear();
        rec.scanned.strings.clear();
        rec.scanned.peaks.clear();
        rec.strings = true;
        rec.last_end = 0;
        rec.next_sym = 0;
        rec.reserved_seen = 0;
        self.start_recording(Some(rec));
    }

    fn start_recording(&mut self, rec: Option<Box<Recording>>) {
        self.rec = rec;
        self.reached();
    }

    /// The recording, with the `<eof>` token (the last one read).
    pub(super) fn stop_recording(&mut self) -> Option<Box<Recording>> {
        let mut rec = self.rec.take()?;
        rec.marks.eof = self.tok_ix;
        Some(rec)
    }

    /// Token `tok_ix` became the current one.
    #[inline]
    pub(super) fn reached(&mut self) {
        if self.rec.is_some() {
            self.reached_recording();
        }
    }

    #[inline(never)]
    fn reached_recording(&mut self) {
        let (ix, tok, sym) = (self.tok_ix, self.tok, self.tok_sym);
        let (src, version) = (self.lex.src(), self.version);
        let r = self.rec.as_deref_mut().expect("recording");
        if r.strings {
            r.scan(version, src, ix, tok, sym);
        }
        if r.target == Some(ix) {
            self.found_target();
        }
    }

    #[cold]
    #[inline(never)]
    fn found_target(&mut self) {
        let found = Found {
            line: self.lex.line(),
            near: self.near(),
            span: self.tok.span,
        };
        self.rec.as_mut().expect("recording").found = Some(found);
    }
}

impl Recording {
    /// Token `ix`, `tok`: how full the scanner's buffer gets reading it,
    /// and the string PUC's scanner makes for it. Up to 5.2 each function
    /// keys the strings in its own table, so every one is kept; later the
    /// chunk's one table takes each string once, so only the first of each.
    #[inline]
    fn scan(&mut self, version: LuaVersion, src: &[u8], ix: u32, tok: LexTok, sym: Sym) {
        use crate::frontend::token::Token as T;
        let span = tok.span;
        let (start, end) = (span.start as usize, span.end as usize);
        // the scanner holds no more than the gap's bytes or the token's
        // and a terminating zero
        if start > self.last_end + 32 || end - start >= 32 {
            self.peak(version, src, ix, span);
        }
        self.last_end = end;
        let reserved = |i: u8| (ScanText::Reserved(i, span), ScanKind::Reserved);
        let (text, kind) = match tok.tok {
            T::Name(()) => (ScanText::Sym(sym), ScanKind::Name),
            T::Str(()) => (ScanText::Sym(sym), ScanKind::Str),
            T::And => reserved(0),
            T::Break => reserved(1),
            T::Do => reserved(2),
            T::Else => reserved(3),
            T::Elseif => reserved(4),
            T::End => reserved(5),
            T::False => reserved(6),
            T::For => reserved(7),
            T::Function => reserved(8),
            T::Global => reserved(9),
            T::Goto => reserved(10),
            T::If => reserved(11),
            T::In => reserved(12),
            T::Local => reserved(13),
            T::Nil => reserved(14),
            T::Not => reserved(15),
            T::Or => reserved(16),
            T::Repeat => reserved(17),
            T::Return => reserved(18),
            T::Then => reserved(19),
            T::True => reserved(20),
            T::Until => reserved(21),
            T::While => reserved(22),
            _ => return,
        };
        if version >= LuaVersion::Lua53 {
            // names are numbered as they first appear
            match text {
                ScanText::Sym(s) if s.0 < self.next_sym => return,
                ScanText::Sym(s) => self.next_sym = s.0 + 1,
                ScanText::Reserved(i, _) if self.reserved_seen & 1 << i != 0 => return,
                ScanText::Reserved(i, _) => self.reserved_seen |= 1 << i,
            }
        }
        self.scanned.strings.push((ix, text, kind));
    }

    #[cold]
    #[inline(never)]
    fn peak(&mut self, version: LuaVersion, src: &[u8], ix: u32, span: Span) {
        let gap = &src[self.last_end.min(span.start as usize)..span.start as usize];
        let peak = scan_buf::peak(version, gap, span.slice(src));
        if peak > 32 {
            self.scanned.peaks.push((ix, peak as u32));
        }
    }
}

impl Parser<'_> {
    /// A statement starts at the current token.
    pub(super) fn open_stat(&mut self) {
        if let Some(r) = &mut self.rec {
            r.marks.open.push((self.tok_ix, r.marks.pending.len()));
        }
    }

    /// The statement opened last was no statement (a lone `;`).
    pub(super) fn drop_stat(&mut self) {
        if let Some(r) = &mut self.rec {
            let (_, start) = r.marks.open.pop().expect("open statement");
            r.marks.pending.truncate(start);
        }
    }

    /// Statement `sid`, pushed, ended before the current token.
    pub(super) fn close_stat(&mut self, sid: StatId) {
        if let Some(r) = &mut self.rec {
            let m = &mut r.marks;
            let (first, start) = m.open.pop().expect("open statement");
            debug_assert_eq!(m.stat_first.len(), sid.0 as usize);
            m.stat_first.push(first);
            m.stat_follow.push(self.tok_ix);
            m.stat_marks
                .push((m.marks.len() as u32, (m.pending.len() - start) as u32));
            m.marks.extend(m.pending.drain(start..));
        }
    }

    /// The current token is one the statement being read marks: an `if`
    /// arm's end (`elseif`, `else` or `end`), `until`, and the `(` of a
    /// function statement's parameters.
    pub(super) fn mark(&mut self) {
        if let Some(r) = &mut self.rec {
            r.marks.pending.push(self.tok_ix);
        }
    }

    /// Expression `eid`, pushed, ended before the current token.
    pub(super) fn close_expr(&mut self, eid: ExprId) {
        if let Some(r) = &mut self.rec {
            debug_assert_eq!(r.marks.expr_follow.len(), eid.0 as usize);
            r.marks.expr_follow.push(self.tok_ix);
        }
    }

    /// Expression `eid` (a table or a function) opened at token `open`.
    pub(super) fn opened_expr(&mut self, eid: ExprId, open: u32) {
        if let Some(r) = &mut self.rec {
            r.marks.expr_open.push((eid.0, open));
        }
    }
}
