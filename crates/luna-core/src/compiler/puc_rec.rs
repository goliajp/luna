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
    /// Log `ev` at token `at`, when the parse recorded its marks.
    #[inline]
    pub(super) fn log_at(&mut self, at: TokRef, ev: PEv) {
        let Some(m) = self.marks else {
            return;
        };
        let last = &mut self.puc_state.last;
        let (tok, named) = if at == last.at {
            (last.tok, last.named)
        } else if at == TokRef::Next {
            self.puc_state.nexts = true;
            (NEXT, At::Other)
        } else {
            let tok = match at {
                TokRef::Start => -1,
                t => i64::from(t.resolve(self.ast, m)),
            };
            *last = LastTok {
                at,
                tok,
                named: At::of(at),
            };
            (tok, last.named)
        };
        self.puc_log.push_or_abort(Rec { tok, at: named, ev });
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
