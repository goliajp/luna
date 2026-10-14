//! What PUC's parser and code generator allocate, logged while luna
//! compiles: each event with the token PUC's scanner is on when it happens
//! (see [`TokRef`]). The load replays the log, merged with the strings the
//! scanner makes, to count the allocations in PUC's order (see
//! `vm/exec/load_acct.rs`).

use super::*;
use crate::frontend::parser::Marks;

/// One allocation-relevant step of PUC's parser or code generator.
#[derive(Clone, Copy)]
pub(crate) enum PEv {
    /// a function starts (`open_func`, with 5.4+'s `addprototype`)
    Open,
    /// the innermost function ends (`close_func`)
    Close,
    /// an instruction, with its line
    Code(u32),
    /// the last instruction taken back (PUC `removelastinstruction`)
    Uncode,
    /// a vararg function's prologue (5.4+ `setvararg`)
    Prologue,
    /// the parser makes a string of its own (`luaX_newliteral`)
    Lit(&'static str),
    /// a new constant, under its scanner-table key (none: PUC adds it
    /// under no key)
    Const(Option<const_map::ConstKey>),
    /// a new upvalue descriptor of the function at this depth (0: the
    /// main function)
    Upval(u32),
    /// a local variable's debug record (`registerlocalvar`)
    LocVar,
    /// an active-variable entry (`new_localvar`): how many there are now
    ActVar(u32),
    /// a pending goto, and a label: how many there are now
    Goto(u32),
    Label(u32),
}

impl<'a> Compiler<'a> {
    /// Log a new active-variable entry, with how many all the functions
    /// being compiled have (PUC's `dyd->actvar` is one array for them all).
    pub(super) fn log_actvar(&mut self) {
        let n: usize = self.open_levels().iter().map(|l| l.avars.len()).sum();
        self.log(PEv::ActVar(n as u32));
    }

    /// The local variables declared since [`Self::defer_locvars`] get
    /// their debug records now (PUC `adjustlocalvars`).
    pub(super) fn flush_locvars(&mut self) {
        for _ in 0..std::mem::take(&mut self.pending_locvars) {
            self.log(PEv::LocVar);
        }
        self.defer_locvars = false;
    }

    /// Log `ev` at the scanner's current token.
    #[inline]
    pub(super) fn log(&mut self, ev: PEv) {
        let at = self.cur;
        self.puc_log.push_or_abort((at, ev));
    }
}

/// The log resolved to token numbers: each event with the token PUC's
/// scanner is on, in order (-1: before the first token). An event logged at
/// [`TokRef::Next`] takes the token of the next event that names one.
pub(crate) fn resolve(log: &[(TokRef, PEv)], ast: &Chunk, m: &Marks) -> Vec<(i64, PEv)> {
    let at = |t: TokRef| match t {
        TokRef::Start => -1,
        t => i64::from(t.resolve(ast, m)),
    };
    let mut out: Vec<(i64, PEv)> = log.iter().map(|&(t, e)| (at(t), e)).collect();
    let mut next = i64::from(m.eof);
    for (i, &(t, _)) in log.iter().enumerate().rev() {
        if t == TokRef::Next {
            out[i].0 = next;
        } else {
            next = out[i].0;
        }
    }
    out
}
