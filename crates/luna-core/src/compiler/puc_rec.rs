//! The log's records: each event of [`super::puc_log`] with the number of
//! the token PUC's scanner is on when it happens, resolved as it is logged
//! against the marks the parse recorded.

use super::puc_log::PEv;
use super::*;

/// The token of an event logged at [`TokRef::Next`] until the compile
/// ends: then the token of the next event that names one.
pub(crate) const NEXT: i64 = i64::MIN;

/// An event and its token (-1: before the scanner reads the first).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rec {
    pub(crate) tok: i64,
    /// the node the token is named by, which a failed load's replay asks
    /// whether the parse read to its end; for an event at
    /// [`TokRef::Next`], that of the event it is made with
    pub(crate) at: At,
    pub(crate) ev: PEv,
}

/// What a token is named by, as a failed load's replay needs it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum At {
    /// the first token of an expression (or what follows from it)
    First,
    /// a token after the start of expression `e`
    Expr(u32),
    /// a token of statement `s`
    Stat(u32),
    /// statement `s`'s last token
    StatLast(u32),
    /// the chunk's start or end
    Other,
}

impl At {
    fn of(t: TokRef) -> At {
        match t {
            TokRef::First(..) => At::First,
            TokRef::After(e, _) | TokRef::Open(e, _) => At::Expr(e.0),
            TokRef::StatAfter(s, -1) => At::StatLast(s.0),
            TokRef::StatFirst(s, _)
            | TokRef::StatAfter(s, _)
            | TokRef::StatMark(s, _)
            | TokRef::MarkPlus(s, ..) => At::Stat(s.0),
            TokRef::Eof | TokRef::Start | TokRef::Next => At::Other,
        }
    }
}

/// The last token resolved, kept for the events after it at the same one.
#[derive(Clone, Copy)]
pub(super) struct LastTok {
    at: TokRef,
    tok: i64,
    named: At,
}

impl Default for LastTok {
    fn default() -> Self {
        LastTok {
            at: TokRef::Start,
            tok: -1,
            named: At::Other,
        }
    }
}

impl Compiler<'_> {
    /// The number and node of token `at`, when the parse recorded its
    /// marks.
    #[inline]
    fn tok_of(&mut self, at: TokRef) -> Option<(i64, At)> {
        let m = self.marks?;
        let last = &mut self.puc_state.last;
        if at == last.at {
            return Some((last.tok, last.named));
        }
        if at == TokRef::Next {
            self.puc_state.nexts = true;
            return Some((NEXT, At::Other));
        }
        let tok = match at {
            TokRef::Start => -1,
            t => i64::from(t.resolve(self.ast, m)),
        };
        *last = LastTok {
            at,
            tok,
            named: At::of(at),
        };
        Some((tok, last.named))
    }

    /// Log `ev` at token `at`.
    #[inline]
    pub(super) fn log_at(&mut self, at: TokRef, ev: PEv) {
        if let Some((tok, named)) = self.tok_of(at) {
            self.puc_log.push_or_abort(Rec { tok, at: named, ev });
        }
    }

    /// Log an instruction on `line` at token `at`: one more of the
    /// instructions just before, when they are at the same token and line
    /// and logged since [`Self::log_seam`].
    #[inline]
    pub(super) fn log_code(&mut self, at: TokRef, line: u32) {
        let Some((tok, named)) = self.tok_of(at) else {
            return;
        };
        let seam = self.puc_state.seam;
        if self.puc_log.len() > seam
            && let Some(prev) = self.puc_log.last_mut()
            && (prev.tok, prev.at) == (tok, named)
            && let PEv::Grow(l, n) = &mut prev.ev
            && *l == line
        {
            *n += 1;
            return;
        }
        let ev = PEv::Grow(line, 1);
        self.puc_log.push_or_abort(Rec { tok, at: named, ev });
    }

    /// Where the log is now: the events logged from here on, which may be
    /// moved, are kept apart from the ones before.
    pub(super) fn log_seam(&mut self) -> usize {
        let n = self.puc_log.len();
        self.puc_state.seam = n;
        n
    }

    /// The compile is over: each event logged at [`TokRef::Next`] takes
    /// the token of the next event that names one (the chunk's end when
    /// none does).
    pub(super) fn settle_log(&mut self) {
        let Some(m) = self.marks.filter(|_| self.puc_state.nexts) else {
            return;
        };
        let mut next = (i64::from(m.eof), At::Other);
        for r in self.puc_log.iter_mut().rev() {
            if r.tok == NEXT {
                (r.tok, r.at) = next;
            } else {
                next = (r.tok, r.at);
            }
        }
    }
}
