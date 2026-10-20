//! What PUC's parser and code generator allocate, logged while luna
//! compiles: each event with the token PUC's scanner is on when it happens
//! (see [`TokRef`]). The load replays the log, merged with the strings the
//! scanner makes, to count the allocations in PUC's order (see
//! `vm/exec/load_acct.rs`).

use super::*;

/// One allocation-relevant step of PUC's parser or code generator.
#[derive(Clone, Copy)]
#[cfg_attr(feature = "gc-trace", derive(Debug))]
pub(crate) enum PEv {
    /// a function starts (`open_func`, with 5.4+'s `addprototype`)
    Open,
    /// the innermost function ends (`close_func`)
    Close,
    /// an instruction, with its line
    Code(u32),
    /// an instruction of the enclosing function while this one is still
    /// open (5.2+ `codeclosure` comes before `close_func`)
    CodeUp(u32),
    /// the last instruction taken back (PUC `removelastinstruction`)
    Uncode,
    /// a vararg function's prologue (5.4+ `setvararg`)
    Prologue,
    /// the parser makes a string of its own (`luaX_newliteral`)
    Lit(&'static str),
    /// a new constant, with luna's key for it (none: PUC adds it under no
    /// key), and its value (from which the replay makes PUC's key)
    Const(Option<const_map::ConstKey>, Value),
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
    /// nothing allocated: the values loaded early are used here (see
    /// [`TokRef::Next`])
    Use,
}

/// The log's state while a function compiles.
#[derive(Default)]
pub(super) struct LogState {
    /// an instruction luna emitted early waits for the token where its
    /// value is used ([`Compiler::use_early`])
    pub(super) early: bool,
    /// locals declared now get their debug records later
    /// ([`Compiler::flush_locvars`])
    pub(super) defer: bool,
    pub(super) pending: u32,
    /// the constant the newest 5.4+ string literal made, in the function
    /// at this depth ([`Compiler::drop_literal`])
    pub(super) fresh_literal: Option<(usize, u32)>,
    /// see [`super::ReplayOpts::lenient`]
    pub(super) lenient: bool,
}

impl<'a> Compiler<'a> {
    /// Log a new active-variable entry, with how many all the functions
    /// being compiled have (PUC's `dyd->actvar` is one array for them all).
    pub(super) fn log_actvar(&mut self) {
        let n = self.actvars();
        self.log(PEv::ActVar(n));
    }

    /// The local variables declared since [`LogState::defer`] was set
    /// get their debug records now (PUC `adjustlocalvars`).
    pub(super) fn flush_locvars(&mut self) {
        for _ in 0..std::mem::take(&mut self.puc_state.pending) {
            self.log(PEv::LocVar);
        }
        self.puc_state.defer = false;
    }

    /// The parser's entries for a new local `name` (PUC `new_localvar`):
    /// its literal string, if the parser makes one, and its active-variable
    /// entry; nothing when [`Self::prelog_localvar`] logged them.
    pub(super) fn log_new_localvar(&mut self, name: &str) {
        if self.lr().prelogged > 0 {
            self.l().prelogged -= 1;
            return;
        }
        if let Some(lit) = puc_literal(name) {
            self.log(PEv::Lit(lit));
        }
        self.log_early_locvar();
        self.log_actvar();
    }

    /// Up to 5.3 `new_localvar` registers the variable's debug record at
    /// once; 5.4 waits for `adjustlocalvars`.
    fn log_early_locvar(&mut self) {
        if self.version <= LuaVersion::Lua53 {
            self.log(PEv::LocVar);
        }
    }

    /// Log the parser's entries for the local `name` at token `at`, where
    /// PUC makes them before the statement's expressions, ahead of the
    /// declaration.
    pub(super) fn prelog_localvar(&mut self, at: TokRef, name: &str) -> Result<(), SyntaxError> {
        let cur = std::mem::replace(&mut self.cur, at);
        // up to 5.4 `new_localvar` checks the cap as each name is read,
        // before the statement registers any of them
        let active = self.lr().avars.iter().filter(|a| a.reg.is_some()).count() as u32;
        if self.version <= LuaVersion::Lua54 && active + self.lr().prelogged >= MAX_LOCALS {
            return Err(self.limit_err("local variables", MAX_LOCALS));
        }
        if let Some(lit) = puc_literal(name) {
            self.log(PEv::Lit(lit));
        }
        self.log_early_locvar();
        let n = self.actvars() + 1;
        self.log(PEv::ActVar(n));
        self.cur = cur;
        self.l().prelogged += 1;
        Ok(())
    }

    /// How many entries PUC's `dyd->actvar` holds: the active variables of
    /// all the functions being compiled, with the ones whose entries are
    /// made ahead of their declaration.
    fn actvars(&self) -> u32 {
        let n: usize = self
            .open_levels()
            .iter()
            .map(|l| l.avars.len() + l.prelogged as usize)
            .sum();
        n as u32
    }

    /// [`Self::prelog_localvar`] for each name of a `local` statement, on
    /// the token after the name.
    pub(super) fn log_local_names(
        &mut self,
        sid: StatId,
        collective: Option<crate::frontend::ast::Attrib>,
        names: &[crate::frontend::ast::AttribName],
    ) -> Result<(), SyntaxError> {
        // `local`, then `<attrib>` before the names (5.5)
        let mut tok = if collective.is_some() { 4 } else { 1 };
        for an in names {
            self.prelog_localvar(TokRef::StatFirst(sid, tok + 1), self.nm(&an.name))?;
            tok += 2 + if an.attrib.is_some() { 3 } else { 0 };
        }
        Ok(())
    }

    /// [`Self::prelog_localvar`] for a `for` loop's hidden locals and
    /// variables, which PUC makes on the token after each variable's name,
    /// the hidden ones with the first.
    pub(super) fn log_for_names(
        &mut self,
        sid: StatId,
        hidden: &[&str],
        vars: &[(&str, bool)],
    ) -> Result<(), SyntaxError> {
        for &h in hidden {
            self.prelog_localvar(TokRef::StatFirst(sid, 2), h)?;
        }
        for (j, &(v, _)) in vars.iter().enumerate() {
            self.prelog_localvar(TokRef::StatFirst(sid, 2 + 2 * j as i32), v)?;
        }
        Ok(())
    }

    /// The constant of string literal `id`. Up to 5.3 PUC adds it while
    /// the literal is the scanner's token (`codestring`; the token after a
    /// key name); from 5.4 only once the value is used (`VKSTR`), which
    /// luna's next step logs.
    pub(super) fn literal_const(&mut self, s: ast::Sym, id: ExprId) -> u32 {
        if self.version < LuaVersion::Lua54 {
            // a name after `.` is read past first (`checkname`)
            let at = if self.ast.is_dot_key(id) {
                TokRef::After(id, 0)
            } else {
                TokRef::First(id, 0)
            };
            let cur = std::mem::replace(&mut self.cur, at);
            let k = self.sym_const(s);
            self.cur = cur;
            return k;
        }
        let cur = std::mem::replace(&mut self.cur, TokRef::Next);
        let n = self.lr().consts.len();
        let k = self.sym_const(s);
        self.cur = cur;
        self.puc_state.early = true;
        self.puc_state.fresh_literal = (k as usize == n).then_some((self.depth, k));
        k
    }

    /// From 5.4 a string literal whose value goes unused makes no constant
    /// (PUC drops the `VKSTR` undischarged): the constant luna made for it,
    /// the function's newest, is taken back.
    pub(super) fn drop_literal(&mut self, e: Exp) {
        let fresh = self.puc_state.fresh_literal.take();
        if let Exp::Const(k) = e
            && fresh == Some((self.depth, k))
            && k as usize + 1 == self.lr().consts.len()
        {
            self.l().consts.pop();
            let log = &mut self.puc_log;
            if let Some(i) = log.iter().rposition(|(_, e)| matches!(e, PEv::Const(..))) {
                log.remove(i);
            }
        }
    }

    /// From 5.2 PUC codes a function's closure in the enclosing function
    /// before it closes the function (`close_func` makes the final return):
    /// the closure instruction just emitted goes before the events from
    /// `final_ret` on, the function's final return and its `Close`.
    pub(super) fn close_after_closure(&mut self, final_ret: usize) {
        let log = &mut self.puc_log;
        let n = log.len();
        if n < 2 || final_ret >= n - 1 || !matches!(log[n - 2].1, PEv::Close) {
            return;
        }
        if let (tok, PEv::Code(line)) = log[n - 1] {
            log[final_ret..].rotate_right(1);
            log[final_ret] = (tok, PEv::CodeUp(line));
        }
    }

    /// The last instruction luna emitted is taken back before PUC would
    /// make it: PUC never codes it.
    pub(super) fn unlog_code(&mut self) {
        let log = &mut self.puc_log;
        if let Some(i) = log.iter().rposition(|(_, e)| matches!(e, PEv::Code(_))) {
            log.remove(i);
        }
    }

    /// The events logged since `from` go before the instructions emitted
    /// early that precede them: PUC makes them before it uses the value.
    pub(super) fn log_before_early(&mut self, from: usize) {
        let log = &mut self.puc_log;
        let start = log[..from]
            .iter()
            .rposition(|(t, _)| *t != TokRef::Next)
            .map_or(0, |i| i + 1);
        log[start..].rotate_left(from - start);
    }

    /// A value is used here: the instructions luna emitted early for it are
    /// PUC's now.
    #[inline]
    pub(super) fn use_early(&mut self) {
        if std::mem::take(&mut self.puc_state.early) {
            self.log(PEv::Use);
        }
    }

    /// Log `ev` at the scanner's current token.
    #[inline]
    pub(super) fn log(&mut self, ev: PEv) {
        let at = self.cur;
        self.puc_log.push_or_abort((at, ev));
    }
}

/// The names PUC's parser makes strings of its own for
/// (`luaX_newliteral`), when it declares a local by them.
fn puc_literal(name: &str) -> Option<&'static str> {
    [
        "self",
        "arg",
        "(for index)",
        "(for limit)",
        "(for step)",
        "(for generator)",
        "(for state)",
        "(for control)",
        "(vararg table)",
    ]
    .into_iter()
    .find(|&l| l == name)
}
