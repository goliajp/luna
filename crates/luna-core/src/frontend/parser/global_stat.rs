//! 5.5 `global` declarations.

use super::*;

impl<'s> Parser<'s> {
    pub(super) fn global_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let line = self.prev_line;
            let name = self.expect_name()?;
            self.declare([name.sym], VarKind::Global);
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::GlobalFunction { name, body }));
        }
        // `global [attrib] '*'`
        let leading = self.attrib()?;
        if self.accept(Token::Star)? {
            self.goto_step(|g| {
                g.declare("*", VarKind::Global);
                Ok(())
            })?;
            return Ok(self.push_stat(Stat::GlobalAll { attrib: leading }));
        }
        let mark = self.stk.attribs.len();
        loop {
            let name = self.expect_name()?;
            let attrib = self.attrib()?;
            self.stk.attribs.push_or_abort(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let names = finish(&mut self.chunk, &mut self.stk.attribs, mark);
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            List::EMPTY
        };
        // the declared names come into scope after their initializers
        self.declare_attrib_names(names, leading, true);
        Ok(self.push_stat(Stat::Global {
            collective: leading,
            names,
            exprs,
        }))
    }
}
