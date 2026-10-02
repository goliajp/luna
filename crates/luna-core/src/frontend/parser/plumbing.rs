//! Token plumbing: advancing, expecting, error construction and nesting depth.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn advance(&mut self) -> Result<LexTok, SyntaxError> {
        self.last_line = self.lex.line();
        let next = match self.peeked.take() {
            Some(t) => t,
            None => self.lex.next_token()?,
        };
        self.prev_line = self.tok.line;
        self.tok_char = next.char;
        self.tok_sym = next.sym;
        Ok(std::mem::replace(&mut self.tok, next.info))
    }

    pub(super) fn peek(&mut self) -> Result<&Tok, SyntaxError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lex.next_token()?);
        }
        Ok(&self.peeked.as_ref().unwrap().info.tok)
    }

    pub(super) fn near(&self) -> Vec<u8> {
        match self.tok_char {
            Some(c) => near_text(self.version, Near::Char(c)),
            // a string's bytes are with the names; a name is shown from
            // the source
            None => self
                .tok
                .tok
                .map(
                    |()| self.lex.names().bytes(self.tok_sym).to_vec(),
                    |()| Box::default(),
                    |()| Box::default(),
                )
                .near_bytes(self.lex.src(), self.tok.span, self.version),
        }
    }

    /// PUC `luaX_syntaxerror`: `msg near <current token>`.
    pub(super) fn error(&self, msg: impl AsRef<str>) -> SyntaxError {
        // a NUL byte comes back from PUC's scanner as token 0, which
        // `lexerror` reads as "no near-token"
        if self.tok_char == Some(0) {
            return self.plain_error(msg.as_ref());
        }
        let mut bytes = msg.as_ref().as_bytes().to_vec();
        bytes.extend_from_slice(b" near ");
        bytes.extend_from_slice(&self.near());
        SyntaxError {
            line: self.lex.line(),
            msg: bytes,
        }
    }

    /// PUC `luaK_semerror` / `luaX_lexerror(.., 0)`: no near-token.
    pub(super) fn plain_error(&self, msg: impl Into<Vec<u8>>) -> SyntaxError {
        SyntaxError {
            line: self.lex.line(),
            msg: msg.into(),
        }
    }

    /// PUC `error_expected`. `what` is a token as `luaX_token2str` spells
    /// it; the `<name>`-style pseudo-tokens are quoted only by 5.1.
    pub(super) fn error_expected(&self, what: &str) -> SyntaxError {
        if what.starts_with('<') && self.version >= LuaVersion::Lua52 {
            self.error(format!("{what} expected"))
        } else {
            self.error(format!("'{what}' expected"))
        }
    }

    pub(super) fn accept(&mut self, tok: Tok) -> Result<bool, SyntaxError> {
        if self.tok.tok == tok {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub(super) fn expect(&mut self, tok: Tok, what: &str) -> Result<(), SyntaxError> {
        if !self.accept(tok)? {
            return Err(self.error_expected(what));
        }
        Ok(())
    }

    /// Like PUC check_match: closing token with a pointer back to the opener.
    pub(super) fn expect_match(
        &mut self,
        tok: Tok,
        what: &str,
        who: &str,
        who_line: u32,
    ) -> Result<(), SyntaxError> {
        if !self.accept(tok)? {
            if who_line == self.lex.line() {
                return Err(self.error_expected(what));
            }
            return Err(self.error(format!(
                "'{what}' expected (to close '{who}' at line {who_line})"
            )));
        }
        Ok(())
    }

    pub(super) fn expect_name(&mut self) -> Result<Name, SyntaxError> {
        if !matches!(self.tok.tok, Token::Name(_)) {
            return Err(self.error_expected("<name>"));
        }
        let sym = self.tok_sym;
        let info = self.advance()?;
        Ok(Name {
            sym,
            line: info.line,
        })
    }

    /// The text of an interned name.
    pub(super) fn text(&self, s: Sym) -> &str {
        self.lex.names().text(s)
    }

    /// Declare locals to the goto checker.
    pub(super) fn declare(&mut self, syms: impl IntoIterator<Item = Sym>) {
        let Parser { gotos, lex, .. } = self;
        if let Some(g) = gotos {
            for s in syms {
                g.declare(lex.names().text(s));
            }
        }
    }

    pub(super) fn enter(&mut self) -> Result<(), SyntaxError> {
        self.depth += 1;
        // 5.1-5.3 `enterlevel` fails past the limit; 5.4+ `luaE_incCstack`
        // at it
        let limit = if self.version >= LuaVersion::Lua54 {
            MAX_DEPTH - 1
        } else {
            MAX_DEPTH
        };
        if self.depth > limit {
            return Err(self.levels_error());
        }
        Ok(())
    }

    /// PUC's nesting-limit error: 5.1 `enterlevel` has its own wording and
    /// no near-token; 5.2/5.3 run it through `errorlimit` ("C levels", in
    /// the function being parsed); 5.4+ count parser levels on the C stack,
    /// and `luaE_checkcstack` raises a runtime error that carries no
    /// position because the running function is `load`, not Lua code.
    pub(super) fn levels_error(&self) -> SyntaxError {
        match self.version {
            LuaVersion::Lua51 => self.plain_error("chunk has too many syntax levels"),
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.error(format!(
                "too many C levels (limit is 200) in {}",
                self.where_()
            )),
            _ => SyntaxError::unpositioned("C stack overflow"),
        }
    }

    /// PUC `errorlimit`'s `where`: the function being parsed.
    pub(super) fn where_(&self) -> String {
        let &(_, line_defined, _) = self.func_local_count.last().expect("func ctx");
        if self.func_local_count.len() == 1 {
            "main function".to_string()
        } else {
            format!("function at line {line_defined}")
        }
    }

    pub(super) fn leave(&mut self) {
        self.depth -= 1;
    }

    pub(super) fn push_expr(&mut self, e: Expr) -> ExprId {
        self.chunk.exprs.push(e);
        ExprId((self.chunk.exprs.len() - 1) as u32)
    }

    pub(super) fn push_stat(&mut self, s: Stat) -> StatId {
        self.chunk.stats.push(s);
        StatId((self.chunk.stats.len() - 1) as u32)
    }

    /// Push a statement that ended with the `end` just read.
    pub(super) fn push_ended_stat(&mut self, s: Stat) -> StatId {
        let id = self.push_stat(s);
        let idx = id.0 as usize;
        self.end_lines.resize(idx + 1, 0);
        self.end_lines[idx] = self.prev_line;
        id
    }
}
