//! Declarations: `function` statements, `local` and their attributes.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn function_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let base = self.expect_name()?;
        let mark = self.stk.names.len();
        while self.accept(Token::Dot)? {
            let n = self.expect_name()?;
            self.stk.names.push_or_abort(n);
        }
        let path = finish(self.chunk, &mut self.stk.names, mark);
        let method = if self.accept(Token::Colon)? {
            Some(self.expect_name()?)
        } else {
            None
        };
        self.mark();
        let body = self.func_body(line)?;
        Ok(self.push_stat(Stat::Function {
            name: FuncName { base, path, method },
            body,
        }))
    }

    pub(super) fn attrib(&mut self) -> Result<Option<Attrib>, SyntaxError> {
        if !(self.version.has_attribs() && self.tok.tok == Token::Lt) {
            return Ok(None);
        }
        self.advance()?;
        let name = self.expect_name()?;
        // PUC reads the `>` before it looks at the name
        self.expect(Token::Gt, ">")?;
        let attrib = match self.text(name.sym) {
            "const" => Attrib::Const,
            "close" => Attrib::Close,
            // a lenient parse (what a failed load made, counted) keeps the
            // place of the name's `<attrib>`; the error cuts what follows
            _ if self.stk.lenient => Attrib::Const,
            other => {
                let msg = format!("unknown attribute '{other}'");
                return Err(self.plain_error(msg));
            }
        };
        Ok(Some(attrib))
    }

    /// `[attrib] Name [attrib] {',' Name [attrib]} ['=' explist]` — shared by
    /// `local` and `global` declarations.
    pub(super) fn attnamelist(&mut self) -> Result<DeclList, SyntaxError> {
        let collective = if self.version.has_collective_attrib() {
            self.attrib()?
        } else {
            None
        };
        let mark = self.stk.attribs.len();
        loop {
            let name = self.expect_name()?;
            self.new_local()?;
            let attrib = self.attrib()?;
            self.stk.attribs.push_or_abort(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let names = finish(self.chunk, &mut self.stk.attribs, mark);
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            List::EMPTY
        };
        Ok((collective, names, exprs))
    }

    pub(super) fn local_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let name = self.expect_name()?;
            // defined on the line of the token after the name
            let line = self.tok.line;
            // `local function f` declares `f` in the enclosing function before
            // the body is parsed (PUC `localfunc`'s pre-declare); count it.
            self.new_local()?;
            self.activate_locals();
            self.add_local_51(name.sym);
            self.declare([name.sym], VarKind::Local);
            self.mark();
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::LocalFunction { name, body }));
        }
        let (collective, names, exprs) = self.attnamelist()?;
        self.activate_locals();
        self.declare_attrib_names(names, collective, false);
        for i in names.range() {
            self.add_local_51(self.chunk.attrib_name_lists[i].name.sym);
        }
        Ok(self.push_stat(Stat::Local {
            collective,
            names,
            exprs,
        }))
    }
}
