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
    /// the token to report, and what was found for it
    target: Option<u32>,
    found: Option<(u32, Vec<u8>)>,
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

/// The line PUC reports an error at when its scanner is on token `ix` of
/// `src`, and that token as `near` shows it.
pub(crate) fn token_at(
    src: &[u8],
    version: LuaVersion,
    c_depth: u32,
    ix: u32,
) -> Result<Option<(u32, Vec<u8>)>, SyntaxError> {
    record(src, version, c_depth, Some(ix)).map(|r| r.found)
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
    p.rec = Some(Box::new(Recording {
        marks: Marks::default(),
        target,
        found: None,
    }));
    p.reached();
    p.main(src.len())?;
    let mut rec = p.rec.take().expect("recording");
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
            && r.target == Some(self.tok_ix)
        {
            let found = (self.lex.line(), self.near());
            self.rec.as_mut().expect("recording").found = Some(found);
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
