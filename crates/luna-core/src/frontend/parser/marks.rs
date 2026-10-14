//! Token positions recorded by a second parse, to place a compile error.
//!
//! PUC generates code while it parses, so an error its code generator
//! raises (a jump or a register past its limit) shows the token the
//! scanner is on and is reported at that token's line. luna compiles the
//! finished tree; the compiler names that token in terms of the tree (a
//! [`crate::compiler::TokRef`]), and a load that fails that way parses the
//! source again, recording where the nodes start and end, to find it.

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
pub(super) struct Recording {
    pub(super) marks: Marks,
    /// with `strings`: each token PUC's scanner makes a string for (a
    /// name, a reserved word, a string literal), its number and bytes
    pub(super) strings: Option<Vec<(u32, Vec<u8>)>>,
    /// with `strings`: the tokens for which the scanner's buffer holds
    /// more than its first 32 bytes, with how many, and where the last
    /// token read ended
    peaks: Vec<(u32, u32)>,
    last_end: usize,
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

/// What the scanner makes, by token: the strings, and how full its buffer
/// gets (see `scan_buf`).
pub(crate) struct Scanned {
    pub(crate) strings: Vec<(u32, Vec<u8>)>,
    pub(crate) peaks: Vec<(u32, u32)>,
}

/// [`record_marks`] with what PUC's scanner makes.
pub(crate) fn record_strings(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
) -> Result<(Marks, Scanned), SyntaxError> {
    let r = record_with(src, version, c_depth, None, true)?;
    let strings = r.strings.unwrap_or_default();
    Ok((
        r.marks,
        Scanned {
            strings,
            peaks: r.peaks,
        },
    ))
}

#[path = "scan_buf.rs"]
mod scan_buf;
pub(crate) use scan_buf::grown as scan_buf_grown;

fn record(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    target: Option<u32>,
) -> Result<Recording, SyntaxError> {
    record_with(src, version, c_depth, target, false)
}

fn record_with(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    target: Option<u32>,
    strings: bool,
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
    p.rec = Some(Box::new(Recording {
        marks: Marks::default(),
        strings: strings.then(Vec::new),
        peaks: Vec::new(),
        last_end: 0,
        target,
        found: None,
    }));
    p.reached();
    let r = p.main(src.len());
    let mut rec = p.rec.take().expect("recording");
    if let Err(e) = r
        && rec.found.is_none()
    {
        return Err(e);
    }
    rec.marks.eof = p.tok_ix;
    drop(p);
    drop(scratch);
    drop(mem);
    Ok(*rec)
}

impl Parser<'_> {
    /// Token `tok_ix` became the current one.
    pub(super) fn reached(&mut self) {
        if let Some(r) = &self.rec
            && r.strings.is_some()
        {
            self.record_string();
        }
        if let Some(r) = &self.rec
            && r.target == Some(self.tok_ix)
        {
            let found = Found {
                line: self.lex.line(),
                near: self.near(),
                span: self.tok.span,
            };
            self.rec.as_mut().expect("recording").found = Some(found);
        }
    }

    /// The current token's string, when PUC's scanner makes one for it.
    fn record_string(&mut self) {
        use crate::frontend::token::Token as T;
        let span = self.tok.span;
        let src = self.lex.src();
        if let Some(r) = self.rec.as_mut() {
            let gap = &src[r.last_end.min(span.start as usize)..span.start as usize];
            let peak = scan_buf::peak(self.version, gap, span.slice(src));
            if peak > 32 {
                r.peaks.push((self.tok_ix, peak as u32));
            }
            r.last_end = span.end as usize;
        }
        let bytes = match self.tok.tok {
            T::Name(()) | T::Str(()) => self.lex.names().bytes(self.tok_sym).to_vec(),
            T::And
            | T::Break
            | T::Do
            | T::Else
            | T::Elseif
            | T::End
            | T::False
            | T::For
            | T::Function
            | T::Global
            | T::Goto
            | T::If
            | T::In
            | T::Local
            | T::Nil
            | T::Not
            | T::Or
            | T::Repeat
            | T::Return
            | T::Then
            | T::True
            | T::Until
            | T::While => self.tok.span.slice(self.lex.src()).to_vec(),
            _ => return,
        };
        let ix = self.tok_ix;
        if let Some(Some(v)) = self.rec.as_mut().map(|r| r.strings.as_mut()) {
            v.push((ix, bytes));
        }
    }

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
