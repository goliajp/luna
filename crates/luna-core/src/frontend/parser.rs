//! Recursive-descent parser; grammar and operator priorities follow PUC
//! lparser.c. Statement/expression nesting is depth-limited like PUC's
//! C-stack guard.

use crate::frontend::ast::*;
use crate::frontend::error::SyntaxError;
use crate::frontend::lexer::Lexer;
use crate::frontend::token::{Token, TokenInfo};
use crate::version::LuaVersion;

/// PUC `LUAI_MAXCCALLS` — the parser's nesting cap. PUC sets it to 200 and
/// increments once per `subexpr`/`funcargs`/`simpleexp`/`block`/`statement`
/// call; luna's `enter()` fires on roughly the same surfaces (statement +
/// sub_expr + suffixedexp + block), so the same 200 budget keeps
/// errors.lua's `testrep` baseline — 190 levels compile, 201 hits the wall.
const MAX_DEPTH: u32 = 200;

/// `(collective attrib, declared names, initializer exprs)` of a declaration.
type DeclList = (Option<Attrib>, Vec<AttribName>, Vec<ExprId>);

/// Binary operator priorities from lparser.c (left, right); right < left
/// means right-associative.
fn bin_priority(op: BinOp) -> (u8, u8) {
    match op {
        BinOp::Or => (1, 1),
        BinOp::And => (2, 2),
        BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge | BinOp::Ne | BinOp::Eq => (3, 3),
        BinOp::BOr => (4, 4),
        BinOp::BXor => (5, 5),
        BinOp::BAnd => (6, 6),
        BinOp::Shl | BinOp::Shr => (7, 7),
        BinOp::Concat => (9, 8),
        BinOp::Add | BinOp::Sub => (10, 10),
        BinOp::Mul | BinOp::Div | BinOp::IDiv | BinOp::Mod => (11, 11),
        BinOp::Pow => (14, 13),
    }
}

const UNARY_PRIORITY: u8 = 12;

fn bin_op_of(tok: &Token) -> Option<BinOp> {
    Some(match tok {
        Token::Plus => BinOp::Add,
        Token::Minus => BinOp::Sub,
        Token::Star => BinOp::Mul,
        Token::Slash => BinOp::Div,
        Token::DSlash => BinOp::IDiv,
        Token::Percent => BinOp::Mod,
        Token::Caret => BinOp::Pow,
        Token::Concat => BinOp::Concat,
        Token::Eq => BinOp::Eq,
        Token::Ne => BinOp::Ne,
        Token::Lt => BinOp::Lt,
        Token::Le => BinOp::Le,
        Token::Gt => BinOp::Gt,
        Token::Ge => BinOp::Ge,
        Token::And => BinOp::And,
        Token::Or => BinOp::Or,
        Token::Amp => BinOp::BAnd,
        Token::Pipe => BinOp::BOr,
        Token::Tilde => BinOp::BXor,
        Token::Shl => BinOp::Shl,
        Token::Shr => BinOp::Shr,
        _ => return None,
    })
}

fn un_op_of(tok: &Token) -> Option<UnOp> {
    Some(match tok {
        Token::Minus => UnOp::Neg,
        Token::Not => UnOp::Not,
        Token::Hash => UnOp::Len,
        Token::Tilde => UnOp::BNot,
        _ => return None,
    })
}

/// Parse a Lua source chunk for the given dialect into an arena AST
/// ([`Chunk`]).
pub fn parse(src: &[u8], version: LuaVersion) -> Result<Chunk, SyntaxError> {
    let mut lex = Lexer::new(src, version);
    let tok = lex.next_token()?;
    let mut p = Parser {
        lex,
        tok,
        peeked: None,
        prev_line: 1,
        exprs: Vec::new(),
        stats: Vec::new(),
        stat_lines: Vec::new(),
        depth: 0,
        version,
        // the main chunk is the bottom-most function context (line 0 → main)
        func_local_count: vec![(0, 0)],
        upval_chain_51: if version <= LuaVersion::Lua51 {
            vec![FnUvSlot {
                line_defined: 0,
                ..Default::default()
            }]
        } else {
            Vec::new()
        },
    };
    let block = p.block()?;
    if p.tok.tok != Token::Eof {
        return Err(p.error("'<eof>' expected"));
    }
    // PUC attributes the main chunk's final return to the last real token's line
    // (its `lastline`), not the <eof> line that may sit on a trailing blank line.
    let end_line = p.prev_line;
    Ok(Chunk {
        exprs: p.exprs,
        stats: p.stats,
        stat_lines: p.stat_lines,
        block,
        end_line,
    })
}

struct Parser<'s> {
    lex: Lexer<'s>,
    tok: TokenInfo,
    peeked: Option<TokenInfo>,
    /// line of the previously consumed token (for the 5.1 ambiguity check)
    prev_line: u32,
    exprs: Vec<Expr>,
    stats: Vec<Stat>,
    /// starting source line of each statement (by StatId), for precise per-
    /// instruction line info in the compiler
    stat_lines: Vec<u32>,
    depth: u32,
    version: LuaVersion,
    /// One entry per function context (main chunk + nested functions): the
    /// running active-local count (PUC `nactvar`) and the function's defining
    /// line so the limit error can render "in function at line N". Pushed by
    /// `func_body`, popped on exit. Without parse-time tracking, errors.lua
    /// :775 would race a later structural error (a missing `end`) and lose.
    func_local_count: Vec<(u32, u32)>,
    /// Parse-time upvalue accounting for PUC 5.1 (errors.lua :238). PUC 5.1's
    /// `singlevaraux` resolves each identifier as it parses and stops at
    /// `MAXUPVAL=60`; luna defers name resolution to the compiler so a stack
    /// of 61 nested `function`s with no `end`s reaches `<eof>` first and the
    /// missing-`end` error wins. Tracking declared locals + accumulated
    /// upvalue names per nested function here lets the same 60-deep chain
    /// trip while we are still inside `foo61`'s body, with the offending
    /// function's defining line on the error. Only populated for 5.1 — 5.2+
    /// goes through `_ENV` (which would itself be an upvalue) and 5.5
    /// tolerates a wider cap.
    upval_chain_51: Vec<FnUvSlot>,
}

#[derive(Default)]
struct FnUvSlot {
    locals: Vec<Box<str>>,
    upvalues: std::collections::HashSet<Box<str>>,
    line_defined: u32,
}

impl<'s> Parser<'s> {
    // ---- token plumbing ----

    fn advance(&mut self) -> Result<TokenInfo, SyntaxError> {
        let next = match self.peeked.take() {
            Some(t) => t,
            None => self.lex.next_token()?,
        };
        self.prev_line = self.tok.line;
        Ok(std::mem::replace(&mut self.tok, next))
    }

    fn peek(&mut self) -> Result<&Token, SyntaxError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lex.next_token()?);
        }
        Ok(&self.peeked.as_ref().unwrap().tok)
    }

    fn near(&self) -> String {
        self.tok
            .tok
            .describe(self.lex.src(), self.tok.span, self.version)
    }

    fn error(&self, msg: impl AsRef<str>) -> SyntaxError {
        let mut bytes = msg.as_ref().as_bytes().to_vec();
        bytes.extend_from_slice(b" near ");
        bytes.extend_from_slice(self.near().as_bytes());
        SyntaxError {
            line: self.tok.line,
            msg: bytes,
        }
    }

    fn accept(&mut self, tok: Token) -> Result<bool, SyntaxError> {
        if self.tok.tok == tok {
            self.advance()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn expect(&mut self, tok: Token, what: &str) -> Result<(), SyntaxError> {
        if !self.accept(tok)? {
            return Err(self.error(format!("'{what}' expected")));
        }
        Ok(())
    }

    /// Like PUC check_match: closing token with a pointer back to the opener.
    fn expect_match(
        &mut self,
        tok: Token,
        what: &str,
        who: &str,
        who_line: u32,
    ) -> Result<(), SyntaxError> {
        if !self.accept(tok)? {
            if who_line == self.tok.line {
                return Err(self.error(format!("'{what}' expected")));
            }
            return Err(self.error(format!(
                "'{what}' expected (to close '{who}' at line {who_line})"
            )));
        }
        Ok(())
    }

    fn expect_name(&mut self) -> Result<Name, SyntaxError> {
        if !matches!(self.tok.tok, Token::Name(_)) {
            return Err(self.error("<name> expected"));
        }
        let info = self.advance()?;
        let Token::Name(text) = info.tok else {
            unreachable!()
        };
        Ok(Name {
            text,
            line: info.line,
        })
    }

    fn enter(&mut self) -> Result<(), SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            // PUC 5.1 `enterlevel`: "chunk has too many syntax levels".
            // 5.2+ `LUAI_MAXCCALLS` overflow: "too many C levels (limit is
            // N) in main function near <token>". errors.lua 5.1 :214 vs
            // 5.4 :650 baseline on each spelling.
            let msg: &[u8] = if self.version <= LuaVersion::Lua51 {
                b"chunk has too many syntax levels"
            } else {
                b"too many C levels (limit is 200) in main function"
            };
            return Err(SyntaxError {
                line: self.tok.line,
                msg: msg.to_vec(),
            });
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn push_expr(&mut self, e: Expr) -> ExprId {
        self.exprs.push(e);
        ExprId((self.exprs.len() - 1) as u32)
    }

    fn push_stat(&mut self, s: Stat) -> StatId {
        self.stats.push(s);
        StatId((self.stats.len() - 1) as u32)
    }

    // ---- blocks & statements ----

    fn block_follow(&self) -> bool {
        matches!(
            self.tok.tok,
            Token::Eof | Token::End | Token::Else | Token::Elseif | Token::Until
        )
    }

    fn block(&mut self) -> Result<Block, SyntaxError> {
        self.enter()?;
        // PUC `leaveblock` restores `nactvar` to the count at block entry, so
        // a block's locals fall out of scope when it ends. Snapshot the count
        // here so the limit check tracks ACTIVE locals (locals.lua opens many
        // short blocks; without this the cap fires spuriously).
        let local_snapshot = self.func_local_count.last().expect("func ctx").0;
        let locals_51_snap = self.snap_locals_51();
        let mut stats = Vec::new();
        loop {
            if self.block_follow() {
                break;
            }
            if self.tok.tok == Token::Return {
                stats.push(self.return_stat()?);
                break;
            }
            if self.tok.tok == Token::Break && self.version.break_is_last_statement() {
                let line = self.tok.line;
                self.advance()?;
                stats.push(self.push_stat(Stat::Break { line }));
                self.accept(Token::Semi)?;
                break;
            }
            if let Some(s) = self.statement()? {
                stats.push(s);
            }
            if !self.version.has_empty_statement() {
                // 5.1: ';' is a separator after a statement, not a statement
                self.accept(Token::Semi)?;
            }
        }
        self.leave();
        self.func_local_count.last_mut().expect("func ctx").0 = local_snapshot;
        self.restore_locals_51(locals_51_snap);
        Ok(Block { stats })
    }

    fn return_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let exprs = if self.block_follow() || self.tok.tok == Token::Semi {
            Vec::new()
        } else {
            self.exprlist()?
        };
        self.accept(Token::Semi)?;
        Ok(self.push_stat(Stat::Return { exprs, line }))
    }

    fn statement(&mut self) -> Result<Option<StatId>, SyntaxError> {
        // PUC's `statement` does not bump `nCcalls` itself — the surrounding
        // `block` does, and nested forms (do/while/if/function/...) each
        // recurse through `block` again. Counting both would double the cost
        // per `do … end` nesting; errors.lua's `testrep("do ", "", " end")`
        // expects 190 levels to compile and 201 to fail at the same wall as
        // the other shapes.
        let start_line = self.tok.line;
        // 5.5 `global` is a contextual keyword: a declaration only when it
        // leads a statement and the next token starts one (name / '*' /
        // function / attribute '<'). Otherwise it is an ordinary identifier
        // (e.g. `global = 1`, `global()`, `return global`).
        if self.version.has_global_decl()
            && matches!(&self.tok.tok, Token::Name(n) if &**n == "global")
            && matches!(
                self.peek()?,
                Token::Name(_) | Token::Star | Token::Function | Token::Lt
            )
        {
            let stat = self.global_stat()?;
            return Ok(Some(stat));
        }
        let stat = match self.tok.tok {
            Token::Semi => {
                if !self.version.has_empty_statement() {
                    return Err(self.error("unexpected symbol"));
                }
                self.advance()?;
                None
            }
            Token::If => Some(self.if_stat()?),
            Token::While => Some(self.while_stat()?),
            Token::Do => {
                let line = self.tok.line;
                self.advance()?;
                let body = self.block()?;
                self.expect_match(Token::End, "end", "do", line)?;
                Some(self.push_stat(Stat::Do(body)))
            }
            Token::For => Some(self.for_stat()?),
            Token::Repeat => Some(self.repeat_stat()?),
            Token::Function => Some(self.function_stat()?),
            Token::Local => Some(self.local_stat()?),
            Token::DColon => {
                self.advance()?;
                let name = self.expect_name()?;
                self.expect(Token::DColon, "::")?;
                Some(self.push_stat(Stat::Label(name)))
            }
            Token::Break => {
                let line = self.tok.line;
                self.advance()?;
                Some(self.push_stat(Stat::Break { line }))
            }
            Token::Goto => {
                self.advance()?;
                let name = self.expect_name()?;
                Some(self.push_stat(Stat::Goto(name)))
            }
            _ => Some(self.expr_stat()?),
        };
        if let Some(sid) = stat {
            let idx = sid.0 as usize;
            if self.stat_lines.len() <= idx {
                self.stat_lines.resize(idx + 1, 0);
            }
            self.stat_lines[idx] = start_line;
        }
        Ok(stat)
    }

    fn if_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let mut arms = Vec::new();
        let cond = self.expr()?;
        let then_line = self.tok.line;
        self.expect(Token::Then, "then")?;
        arms.push((cond, then_line, self.block()?));
        while self.tok.tok == Token::Elseif {
            self.advance()?;
            let cond = self.expr()?;
            let then_line = self.tok.line;
            self.expect(Token::Then, "then")?;
            arms.push((cond, then_line, self.block()?));
        }
        let else_body = if self.accept(Token::Else)? {
            Some(self.block()?)
        } else {
            None
        };
        self.expect_match(Token::End, "end", "if", line)?;
        Ok(self.push_stat(Stat::If { arms, else_body }))
    }

    fn while_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let cond = self.expr()?;
        self.expect(Token::Do, "do")?;
        let body = self.block()?;
        self.expect_match(Token::End, "end", "while", line)?;
        Ok(self.push_stat(Stat::While { cond, body }))
    }

    fn repeat_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let body = self.block()?;
        self.expect_match(Token::Until, "until", "repeat", line)?;
        let cond = self.expr()?;
        Ok(self.push_stat(Stat::Repeat { body, cond }))
    }

    fn for_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let first = self.expect_name()?;
        match self.tok.tok {
            Token::Assign => {
                self.advance()?;
                let start = self.expr()?;
                self.expect(Token::Comma, ",")?;
                let limit = self.expr()?;
                let step = if self.accept(Token::Comma)? {
                    Some(self.expr()?)
                } else {
                    None
                };
                self.expect(Token::Do, "do")?;
                self.add_local_51(&first.text);
                let body = self.block()?;
                self.expect_match(Token::End, "end", "for", line)?;
                Ok(self.push_stat(Stat::NumericFor {
                    var: first,
                    start,
                    limit,
                    step,
                    body,
                }))
            }
            Token::Comma | Token::In => {
                let mut vars = vec![first];
                while self.accept(Token::Comma)? {
                    vars.push(self.expect_name()?);
                }
                self.expect(Token::In, "in")?;
                let expr_line = self.tok.line;
                let exprs = self.exprlist()?;
                self.expect(Token::Do, "do")?;
                for v in &vars {
                    self.add_local_51(&v.text);
                }
                let body = self.block()?;
                self.expect_match(Token::End, "end", "for", line)?;
                Ok(self.push_stat(Stat::GenericFor {
                    vars,
                    exprs,
                    body,
                    expr_line,
                }))
            }
            _ => Err(self.error("'=' or 'in' expected")),
        }
    }

    fn function_stat(&mut self) -> Result<StatId, SyntaxError> {
        let line = self.tok.line;
        self.advance()?;
        let base = self.expect_name()?;
        let mut path = Vec::new();
        while self.accept(Token::Dot)? {
            path.push(self.expect_name()?);
        }
        let method = if self.accept(Token::Colon)? {
            Some(self.expect_name()?)
        } else {
            None
        };
        let body = self.func_body(line)?;
        Ok(self.push_stat(Stat::Function {
            name: FuncName { base, path, method },
            body,
        }))
    }

    fn attrib(&mut self) -> Result<Option<Attrib>, SyntaxError> {
        if !(self.version.has_attribs() && self.tok.tok == Token::Lt) {
            return Ok(None);
        }
        self.advance()?;
        let name = self.expect_name()?;
        let attrib = match &*name.text {
            "const" => Attrib::Const,
            "close" => Attrib::Close,
            other => {
                return Err(SyntaxError {
                    line: name.line,
                    msg: format!("unknown attribute '{other}'").into_bytes(),
                });
            }
        };
        self.expect(Token::Gt, ">")?;
        Ok(Some(attrib))
    }

    /// `[attrib] Name [attrib] {',' Name [attrib]} ['=' explist]` — shared by
    /// `local` and `global` declarations.
    fn attnamelist(&mut self) -> Result<DeclList, SyntaxError> {
        let collective = if self.version.has_collective_attrib() {
            self.attrib()?
        } else {
            None
        };
        let mut names = Vec::new();
        loop {
            let name = self.expect_name()?;
            let attrib = self.attrib()?;
            names.push(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            Vec::new()
        };
        Ok((collective, names, exprs))
    }

    fn local_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let line = self.prev_line;
            let name = self.expect_name()?;
            // `local function f` declares `f` in the enclosing function before
            // the body is parsed (PUC `localfunc`'s pre-declare); count it.
            self.bump_locals(1)?;
            self.add_local_51(&name.text);
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::LocalFunction { name, body }));
        }
        let (collective, names, exprs) = self.attnamelist()?;
        self.bump_locals(names.len() as u32)?;
        for an in &names {
            self.add_local_51(&an.name.text);
        }
        Ok(self.push_stat(Stat::Local {
            collective,
            names,
            exprs,
        }))
    }

    fn global_stat(&mut self) -> Result<StatId, SyntaxError> {
        self.advance()?;
        if self.accept(Token::Function)? {
            let line = self.prev_line;
            let name = self.expect_name()?;
            let body = self.func_body(line)?;
            return Ok(self.push_stat(Stat::GlobalFunction { name, body }));
        }
        // `global [attrib] '*'`
        let leading = self.attrib()?;
        if self.accept(Token::Star)? {
            return Ok(self.push_stat(Stat::GlobalAll { attrib: leading }));
        }
        let mut names = Vec::new();
        loop {
            let name = self.expect_name()?;
            let attrib = self.attrib()?;
            names.push(AttribName { name, attrib });
            if !self.accept(Token::Comma)? {
                break;
            }
        }
        let exprs = if self.accept(Token::Assign)? {
            self.exprlist()?
        } else {
            Vec::new()
        };
        Ok(self.push_stat(Stat::Global {
            collective: leading,
            names,
            exprs,
        }))
    }

    fn expr_stat(&mut self) -> Result<StatId, SyntaxError> {
        let first = self.suffixed_expr()?;
        if matches!(self.tok.tok, Token::Assign | Token::Comma) {
            let mut targets = vec![first];
            while self.accept(Token::Comma)? {
                // PUC's `restassign` enforces `nvars + nCcalls < LUAI_MAXCCALLS`
                // (200) at each comma; otherwise a runaway multi-assign would
                // exhaust the C stack. errors.lua :650 builds a 500-target list
                // and expects the limit error.
                if targets.len() >= 200 {
                    let msg: &[u8] = if self.version <= LuaVersion::Lua51 {
                        b"chunk has too many syntax levels"
                    } else {
                        b"too many C levels (limit is 200) in main function"
                    };
                    return Err(SyntaxError {
                        line: self.tok.line,
                        msg: msg.to_vec(),
                    });
                }
                targets.push(self.suffixed_expr()?);
            }
            self.expect(Token::Assign, "=")?;
            for &t in &targets {
                if !matches!(self.exprs[t.0 as usize], Expr::Name(_) | Expr::Index { .. }) {
                    return Err(self.error("syntax error"));
                }
            }
            let exprs = self.exprlist()?;
            return Ok(self.push_stat(Stat::Assign { targets, exprs }));
        }
        if !matches!(
            self.exprs[first.0 as usize],
            Expr::Call { .. } | Expr::MethodCall { .. }
        ) {
            return Err(self.error("syntax error"));
        }
        Ok(self.push_stat(Stat::Call(first)))
    }

    // ---- expressions ----

    fn exprlist(&mut self) -> Result<Vec<ExprId>, SyntaxError> {
        let mut list = vec![self.expr()?];
        while self.accept(Token::Comma)? {
            list.push(self.expr()?);
        }
        Ok(list)
    }

    fn expr(&mut self) -> Result<ExprId, SyntaxError> {
        self.sub_expr(0)
    }

    fn sub_expr(&mut self, limit: u8) -> Result<ExprId, SyntaxError> {
        self.enter()?;
        let mut left = if let Some(op) = un_op_of(&self.tok.tok) {
            let line = self.tok.line;
            self.advance()?;
            let operand = self.sub_expr(UNARY_PRIORITY)?;
            self.push_expr(Expr::UnOp { op, operand, line })
        } else {
            self.simple_expr()?
        };
        while let Some(op) = bin_op_of(&self.tok.tok) {
            let (lp, rp) = bin_priority(op);
            if lp <= limit {
                break;
            }
            let line = self.tok.line;
            self.advance()?;
            let rhs = self.sub_expr(rp)?;
            left = self.push_expr(Expr::BinOp {
                op,
                lhs: left,
                rhs,
                line,
            });
        }
        self.leave();
        Ok(left)
    }

    fn simple_expr(&mut self) -> Result<ExprId, SyntaxError> {
        let e = match &self.tok.tok {
            Token::Nil => {
                self.advance()?;
                Expr::Nil
            }
            Token::True => {
                self.advance()?;
                Expr::True
            }
            Token::False => {
                self.advance()?;
                Expr::False
            }
            Token::Ellipsis => {
                self.advance()?;
                Expr::Vararg
            }
            Token::Int(_) => {
                let Token::Int(v) = self.advance()?.tok else {
                    unreachable!()
                };
                Expr::Int(v)
            }
            Token::Float(_) => {
                let Token::Float(v) = self.advance()?.tok else {
                    unreachable!()
                };
                Expr::Float(v)
            }
            Token::Str(_) => {
                let Token::Str(s) = self.advance()?.tok else {
                    unreachable!()
                };
                Expr::Str(s)
            }
            Token::LBrace => return self.table_constructor(),
            Token::Function => {
                let line = self.tok.line;
                self.advance()?;
                Expr::Function(self.func_body(line)?)
            }
            _ => return self.suffixed_expr(),
        };
        Ok(self.push_expr(e))
    }

    fn primary_expr(&mut self) -> Result<ExprId, SyntaxError> {
        match &self.tok.tok {
            Token::Name(_) => {
                let name = self.expect_name()?;
                self.ident_lookup_51(&name.text)?;
                Ok(self.push_expr(Expr::Name(name)))
            }
            Token::LParen => {
                let line = self.tok.line;
                self.advance()?;
                let inner = self.expr()?;
                self.expect_match(Token::RParen, ")", "(", line)?;
                Ok(self.push_expr(Expr::Paren(inner)))
            }
            _ => Err(self.error("unexpected symbol")),
        }
    }

    fn suffixed_expr(&mut self) -> Result<ExprId, SyntaxError> {
        // PUC's `suffixedexp` does *not* bump `nCcalls` on its own — only its
        // callers do (subexpr, funcargs, …). Doing so here too would
        // double-count `(` nesting, since `simpleexp`'s default branch dives
        // through `suffixed_expr` → `primary_expr` → `expr()` → `sub_expr`
        // (which already enters). Keeping the entry out lets errors.lua's
        // `testrep("(")` (paren-only nesting) hit the same 200-level wall as
        // `{`/`,` nesting.
        // PUC 5.1–5.3 `suffixedexp` captured the line of the *primary*
        // expression once and pinned every chained call/method to it
        // (`a\n(...)` → error reports on `a`'s line); 5.4 switched to
        // tracking the current line at each suffix and reports on the `(`
        // line instead. errors.lua's `lineerror` covers both:
        //   - 5.3:  `lineerror([[a\n(\n23)]], 1)` — expects `a`'s line
        //   - 5.4+: `lineerror([[a\n(...)\n23)]], 2)` — expects `(`'s line
        let primary_line = self.tok.line;
        let mut e = self.primary_expr()?;
        loop {
            match &self.tok.tok {
                Token::Dot => {
                    self.advance()?;
                    let name = self.expect_name()?;
                    let key = self.push_expr(Expr::Str(name.text.into_boxed_bytes().into_vec()));
                    e = self.push_expr(Expr::Index { obj: e, key });
                }
                Token::LBracket => {
                    self.advance()?;
                    let key = self.expr()?;
                    self.expect(Token::RBracket, "]")?;
                    e = self.push_expr(Expr::Index { obj: e, key });
                }
                Token::Colon => {
                    self.advance()?;
                    let method = self.expect_name()?;
                    let line = if self.version <= LuaVersion::Lua53 {
                        primary_line
                    } else {
                        self.tok.line
                    };
                    let args = self.call_args()?;
                    e = self.push_expr(Expr::MethodCall {
                        obj: e,
                        method,
                        args,
                        line,
                    });
                }
                Token::LParen | Token::Str(_) | Token::LBrace => {
                    let line = if self.version <= LuaVersion::Lua53 {
                        primary_line
                    } else {
                        self.tok.line
                    };
                    let args = self.call_args()?;
                    e = self.push_expr(Expr::Call {
                        func: e,
                        args,
                        line,
                    });
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> Result<Vec<ExprId>, SyntaxError> {
        match &self.tok.tok {
            Token::LParen => {
                // 5.1 rejects a call paren on a new line (removed in 5.2)
                if self.version == LuaVersion::Lua51 && self.tok.line != self.prev_line {
                    return Err(self.error("ambiguous syntax (function call x new statement)"));
                }
                let line = self.tok.line;
                self.advance()?;
                let args = if self.tok.tok == Token::RParen {
                    Vec::new()
                } else {
                    self.exprlist()?
                };
                self.expect_match(Token::RParen, ")", "(", line)?;
                Ok(args)
            }
            Token::Str(_) => {
                let Token::Str(s) = self.advance()?.tok else {
                    unreachable!()
                };
                Ok(vec![self.push_expr(Expr::Str(s))])
            }
            Token::LBrace => Ok(vec![self.table_constructor()?]),
            _ => Err(self.error("function arguments expected")),
        }
    }

    fn table_constructor(&mut self) -> Result<ExprId, SyntaxError> {
        let line = self.tok.line;
        self.expect(Token::LBrace, "{")?;
        let mut fields = Vec::new();
        loop {
            if self.tok.tok == Token::RBrace {
                break;
            }
            if self.tok.tok == Token::LBracket {
                self.advance()?;
                let key = self.expr()?;
                self.expect(Token::RBracket, "]")?;
                self.expect(Token::Assign, "=")?;
                let value = self.expr()?;
                fields.push(TableField::Keyed(key, value));
            } else if matches!(self.tok.tok, Token::Name(_)) && *self.peek()? == Token::Assign {
                let name = self.expect_name()?;
                self.advance()?; // '='
                let value = self.expr()?;
                fields.push(TableField::Named(name, value));
            } else {
                fields.push(TableField::Item(self.expr()?));
            }
            if !(self.accept(Token::Comma)? || self.accept(Token::Semi)?) {
                break;
            }
        }
        self.expect_match(Token::RBrace, "}", "{", line)?;
        Ok(self.push_expr(Expr::Table { fields, line }))
    }

    // ---- functions ----

    fn func_body(&mut self, line: u32) -> Result<FuncBody, SyntaxError> {
        self.expect(Token::LParen, "(")?;
        self.func_local_count.push((0, line));
        self.enter_fn_51(line);
        let mut params = Vec::new();
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
                            self.add_local_51(&n.text);
                        }
                        break;
                    }
                    Token::Name(_) => {
                        let p = self.expect_name()?;
                        self.add_local_51(&p.text);
                        params.push(p);
                    }
                    _ => return Err(self.error("<name> expected")),
                }
                if !self.accept(Token::Comma)? {
                    break;
                }
            }
        }
        self.expect(Token::RParen, ")")?;
        // params count against the function's local cap (PUC `new_localvar`
        // for parameters); errors raised at this point still attribute to
        // the function's defining line.
        let nparams = params.len() as u32 + matches!(vararg, Vararg::Named(_)) as u32;
        self.bump_locals(nparams)?;
        let block = self.block()?;
        let end_line = self.tok.line; // the `end` token's line, before consuming
        self.expect_match(Token::End, "end", "function", line)?;
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

    fn track_uv_51(&self) -> bool {
        !self.upval_chain_51.is_empty()
    }

    fn add_local_51(&mut self, name: &str) {
        if self.track_uv_51() {
            self.upval_chain_51
                .last_mut()
                .expect("fn ctx")
                .locals
                .push(name.into());
        }
    }

    fn snap_locals_51(&self) -> usize {
        if self.track_uv_51() {
            self.upval_chain_51.last().expect("fn ctx").locals.len()
        } else {
            0
        }
    }

    fn restore_locals_51(&mut self, snap: usize) {
        if self.track_uv_51() {
            self.upval_chain_51
                .last_mut()
                .expect("fn ctx")
                .locals
                .truncate(snap);
        }
    }

    fn enter_fn_51(&mut self, line_defined: u32) {
        if self.track_uv_51() {
            self.upval_chain_51.push(FnUvSlot {
                line_defined,
                ..Default::default()
            });
        }
    }

    fn leave_fn_51(&mut self) {
        if self.track_uv_51() {
            self.upval_chain_51.pop();
        }
    }

    /// PUC 5.1 `singlevaraux`-equivalent: resolve `name` against the current
    /// nested-function stack of declared locals, accumulating an upvalue entry
    /// in every intermediate function between the referencing site and the
    /// owning scope. Returns the PUC "too many upvalues" error the moment a
    /// link's upvalue set crosses 60. No-op for non-5.1 dialects.
    fn ident_lookup_51(&mut self, name: &str) -> Result<(), SyntaxError> {
        if !self.track_uv_51() {
            return Ok(());
        }
        const MAXUPVAL: usize = 60;
        let n = self.upval_chain_51.len();
        let mut owner: Option<usize> = None;
        for k in (0..n).rev() {
            if self.upval_chain_51[k]
                .locals
                .iter()
                .any(|s| s.as_ref() == name)
            {
                owner = Some(k);
                break;
            }
        }
        let Some(owner_idx) = owner else {
            return Ok(());
        };
        if owner_idx + 1 == n {
            return Ok(());
        }
        for k in (owner_idx + 1)..n {
            let inserted = self.upval_chain_51[k].upvalues.insert(name.into());
            if inserted && self.upval_chain_51[k].upvalues.len() > MAXUPVAL {
                let line_defined = self.upval_chain_51[k].line_defined;
                let where_ = if k == 0 {
                    "main function".to_string()
                } else {
                    format!("function at line {line_defined}")
                };
                return Err(SyntaxError {
                    line: self.tok.line,
                    msg: format!("too many upvalues (limit is {MAXUPVAL}) in {where_}")
                        .into_bytes(),
                });
            }
        }
        Ok(())
    }

    /// Increment the active-local count of the function we are currently
    /// parsing and raise PUC's "too many local variables" error if the cap
    /// is exceeded. The "in function at line N" suffix uses the function's
    /// defining line that `func_body` stashed alongside the counter.
    fn bump_locals(&mut self, n: u32) -> Result<(), SyntaxError> {
        const MAXVARS: u32 = 200;
        let depth = self.func_local_count.len();
        let &(cur, line_defined) = self.func_local_count.last().expect("func ctx pushed");
        let new = cur.saturating_add(n);
        if new > MAXVARS {
            let where_ = if depth == 1 {
                "main function".to_string()
            } else {
                format!("function at line {line_defined}")
            };
            return Err(SyntaxError {
                line: self.tok.line,
                msg: format!("too many local variables (limit is {MAXVARS}) in {where_}")
                    .into_bytes(),
            });
        }
        self.func_local_count.last_mut().unwrap().0 = new;
        Ok(())
    }
}
