//! Function bodies and local variable activation.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn func_body(&mut self, line: u32) -> Result<FuncBody, SyntaxError> {
        self.expect(Token::LParen, "(")?;
        self.func_local_count.push((0, line, 0))?;
        self.enter_fn_51(line)?;
        let mark = self.stk.names.len();
        let mut vararg = Vararg::None;
        if self.tok.tok != Token::RParen {
            loop {
                match &self.tok.tok {
                    Token::Ellipsis => {
                        self.advance()?;
                        vararg = if self.version.has_named_vararg()
                            && matches!(self.tok.tok, Token::Name(_))
                        {
                            Vararg::Named(self.expect_name()?)
                        } else {
                            Vararg::Anonymous
                        };
                        if let Vararg::Named(ref n) = vararg {
                            self.new_local()?;
                            self.add_local_51(n.sym)?;
                        }
                        break;
                    }
                    Token::Name(_) => {
                        let p = self.expect_name()?;
                        self.new_local()?;
                        self.add_local_51(p.sym)?;
                        self.stk.names.push(p)?;
                    }
                    _ => return Err(self.error("<name> or '...' expected")),
                }
                if !self.accept(Token::Comma)? {
                    break;
                }
            }
        }
        self.activate_locals()?;
        self.goto_step(|g| Ok(g.enter_function()?))?;
        let params = finish(&mut self.chunk, &mut self.stk.names, mark)?;
        let Parser {
            gotos, lex, chunk, ..
        } = self;
        if let Some(g) = gotos {
            for p in chunk.list(params) {
                g.declare(lex.names().text(p.sym), VarKind::Local)?;
            }
            // 5.5's named `...` parameter is a read-only local
            if let Vararg::Named(n) = &vararg {
                g.declare(lex.names().text(n.sym), VarKind::Const)?;
            }
        }
        self.expect(Token::RParen, ")")?;
        self.funcs.push(FnFlow {
            vararg: !matches!(vararg, Vararg::None),
            loops: 0,
        })?;
        let block = self.block()?;
        let end_line = self.tok.line; // the `end` token's line, before consuming
        self.expect_match(Token::End, "end", "function", line)?;
        self.close_function()?;
        self.func_local_count.pop();
        self.leave_fn_51();
        Ok(FuncBody {
            params,
            vararg,
            block,
            line,
            end_line,
        })
    }

    /// PUC `new_localvar`, right after a local's name is read: up to 5.4
    /// the local cap counts declared-but-pending names too, and the error
    /// points at the token after the name (5.1 words it differently and
    /// shows no token).
    pub(super) fn new_local(&mut self) -> Result<(), SyntaxError> {
        let &(active, line_defined, pending) = self.func_local_count.last().expect("func ctx");
        if self.version <= LuaVersion::Lua54 && active + pending + 1 > MAXVARS {
            return Err(if self.version <= LuaVersion::Lua51 {
                let what = if self.func_local_count.len() == 1 {
                    "main function".to_string()
                } else {
                    format!("function at line {line_defined}")
                };
                self.plain_error(format!("{what} has more than {MAXVARS} local variables"))
            } else {
                self.local_limit_error()
            });
        }
        self.func_local_count.last_mut().expect("func ctx").2 += 1;
        Ok(())
    }

    /// PUC `adjustlocalvars`: the pending locals come into scope. 5.5 checks
    /// the cap here instead of at declaration.
    pub(super) fn activate_locals(&mut self) -> Result<(), SyntaxError> {
        let (active, _, pending) = *self.func_local_count.last().expect("func ctx");
        if self.version >= LuaVersion::Lua55 && active + pending > MAXVARS {
            return Err(self.local_limit_error());
        }
        let slot = self.func_local_count.last_mut().expect("func ctx");
        slot.0 += pending;
        slot.2 = 0;
        Ok(())
    }

    pub(super) fn local_limit_error(&self) -> SyntaxError {
        self.error(format!(
            "too many local variables (limit is {MAXVARS}) in {}",
            self.where_()
        ))
    }
}
