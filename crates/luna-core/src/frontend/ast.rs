//! Arena AST: nodes live in flat vectors inside [`Chunk`], referenced by
//! typed 4-byte ids. Dense storage, no per-node boxing.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExprId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatId(pub u32);

#[derive(Clone, Debug)]
pub struct Name {
    pub text: Box<str>,
    pub line: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attrib {
    Const,
    Close,
}

/// One declared name with its optional `<attrib>`.
#[derive(Clone, Debug)]
pub struct AttribName {
    pub name: Name,
    pub attrib: Option<Attrib>,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stats: Vec<StatId>,
}

/// `function a.b.c:m() ...` target path.
#[derive(Clone, Debug)]
pub struct FuncName {
    pub base: Name,
    pub path: Vec<Name>,
    pub method: Option<Name>,
}

#[derive(Clone, Debug)]
pub enum Vararg {
    None,
    Anonymous,
    /// 5.5 named vararg table: `function f(...t)`.
    Named(Name),
}

#[derive(Clone, Debug)]
pub struct FuncBody {
    pub params: Vec<Name>,
    pub vararg: Vararg,
    pub block: Block,
    pub line: u32,
    /// line of the closing `end` (PUC `lastlinedefined`)
    pub end_line: u32,
}

#[derive(Clone, Debug)]
pub enum Stat {
    Do(Block),
    While {
        cond: ExprId,
        body: Block,
    },
    Repeat {
        body: Block,
        cond: ExprId,
    },
    If {
        /// `(condition, then_line, body)` for the `if` and each `elseif`. The
        /// `then_line` is the source line of the `then` keyword for that arm
        /// — PUC 5.3 attributes the conditional-skip JMP to that line so a
        /// taken if-then-else fires a line hook for the `then` keyword before
        /// the body (`for i=1,n do … then … end` traces include the `then`
        /// keyword line). 5.4 collapsed that back to the body's first line;
        /// see `if_stat` in the compiler for the version split.
        arms: Vec<(ExprId, u32, Block)>,
        else_body: Option<Block>,
    },
    NumericFor {
        var: Name,
        start: ExprId,
        limit: ExprId,
        step: Option<ExprId>,
        body: Block,
    },
    GenericFor {
        vars: Vec<Name>,
        exprs: Vec<ExprId>,
        body: Block,
        /// Line of the first token after `in` (PUC `forlist` `line`); used to
        /// attribute the per-iteration `TFORCALL` so a non-callable iterator
        /// (`for k,v in 3 do …`) raises on the EXPR's source line, not the
        /// `for` line.
        expr_line: u32,
    },
    Local {
        collective: Option<Attrib>,
        names: Vec<AttribName>,
        exprs: Vec<ExprId>,
    },
    /// 5.5 `global` declaration.
    Global {
        collective: Option<Attrib>,
        names: Vec<AttribName>,
        exprs: Vec<ExprId>,
    },
    /// 5.5 `global [attrib] *`.
    GlobalAll {
        attrib: Option<Attrib>,
    },
    Assign {
        targets: Vec<ExprId>,
        exprs: Vec<ExprId>,
    },
    Call(ExprId),
    Function {
        name: FuncName,
        body: FuncBody,
    },
    LocalFunction {
        name: Name,
        body: FuncBody,
    },
    /// 5.5 `global function f() ...`.
    GlobalFunction {
        name: Name,
        body: FuncBody,
    },
    Return {
        exprs: Vec<ExprId>,
        line: u32,
    },
    Break {
        line: u32,
    },
    Goto(Name),
    Label(Name),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    IDiv,
    Mod,
    Pow,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    Neg,
    Not,
    Len,
    BNot,
}

#[derive(Clone, Debug)]
pub enum TableField {
    /// positional `expr`
    Item(ExprId),
    /// `name = expr`
    Named(Name, ExprId),
    /// `[key] = expr`
    Keyed(ExprId, ExprId),
}

#[derive(Clone, Debug)]
pub enum Expr {
    Nil,
    True,
    False,
    Vararg,
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Name(Name),
    /// `obj.key` and `obj[key]` (dot keys become string-literal keys).
    Index {
        obj: ExprId,
        key: ExprId,
    },
    Call {
        func: ExprId,
        args: Vec<ExprId>,
        line: u32,
    },
    MethodCall {
        obj: ExprId,
        method: Name,
        args: Vec<ExprId>,
        line: u32,
    },
    Function(FuncBody),
    Table {
        fields: Vec<TableField>,
        line: u32,
    },
    BinOp {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        line: u32,
    },
    UnOp {
        op: UnOp,
        operand: ExprId,
        line: u32,
    },
    /// Parenthesized expression: truncates multiple results to one.
    Paren(ExprId),
}

/// A parsed chunk: the top-level block plus the node arenas.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub exprs: Vec<Expr>,
    pub stats: Vec<Stat>,
    /// starting source line of each statement, indexed by `StatId`
    pub stat_lines: Vec<u32>,
    pub block: Block,
    /// line of the final `<eof>` token (PUC main-chunk `lastlinedefined`); the
    /// implicit final return is attributed here
    pub end_line: u32,
}

impl Chunk {
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }

    pub fn stat(&self, id: StatId) -> &Stat {
        &self.stats[id.0 as usize]
    }

    /// Starting source line of statement `id` (0 if unrecorded).
    pub fn stat_line(&self, id: StatId) -> u32 {
        self.stat_lines.get(id.0 as usize).copied().unwrap_or(0)
    }
}

/// P11-S5d.N — does any expression in `block` (and nested control-flow,
/// but NOT nested `Expr::Function` bodies) use `Expr::Vararg`?
///
/// PUC 5.1 `LUAI_COMPAT_VARARG` heuristic: a `(...)` function gets a
/// hidden `arg` local UNLESS the body references `...`. The clear of
/// `VARARG_NEEDSARG` in lparser.c happens at `simpleexp`'s TK_DOTS
/// branch, which is a body-level decision. luna's compiler now runs
/// this AST walk before declaring the auto-`arg` local.
pub fn block_uses_vararg(chunk: &Chunk, block: &Block) -> bool {
    block.stats.iter().any(|&sid| stat_uses_vararg(chunk, chunk.stat(sid)))
}

fn stat_uses_vararg(chunk: &Chunk, stat: &Stat) -> bool {
    use Stat::*;
    match stat {
        Do(b) => block_uses_vararg(chunk, b),
        While { cond, body } => {
            expr_uses_vararg(chunk, *cond) || block_uses_vararg(chunk, body)
        }
        Repeat { body, cond } => {
            block_uses_vararg(chunk, body) || expr_uses_vararg(chunk, *cond)
        }
        If { arms, else_body } => {
            arms.iter().any(|(c, _, b)| {
                expr_uses_vararg(chunk, *c) || block_uses_vararg(chunk, b)
            }) || else_body.as_ref().is_some_and(|b| block_uses_vararg(chunk, b))
        }
        NumericFor { start, limit, step, body, .. } => {
            expr_uses_vararg(chunk, *start)
                || expr_uses_vararg(chunk, *limit)
                || step.is_some_and(|s| expr_uses_vararg(chunk, s))
                || block_uses_vararg(chunk, body)
        }
        GenericFor { exprs, body, .. } => {
            exprs.iter().any(|&e| expr_uses_vararg(chunk, e))
                || block_uses_vararg(chunk, body)
        }
        Local { exprs, .. } | Global { exprs, .. } => {
            exprs.iter().any(|&e| expr_uses_vararg(chunk, e))
        }
        GlobalAll { .. } => false,
        Assign { targets, exprs } => {
            targets.iter().any(|&e| expr_uses_vararg(chunk, e))
                || exprs.iter().any(|&e| expr_uses_vararg(chunk, e))
        }
        Call(e) => expr_uses_vararg(chunk, *e),
        // Nested functions own their own vararg context — don't peek
        // inside them. (PUC's `simpleexp` only clears NEEDSARG on
        // direct `...` use in the current function's source.)
        Function { .. } | LocalFunction { .. } | GlobalFunction { .. } => false,
        Return { exprs, .. } => exprs.iter().any(|&e| expr_uses_vararg(chunk, e)),
        Break { .. } | Goto(_) | Label(_) => false,
    }
}

fn expr_uses_vararg(chunk: &Chunk, eid: ExprId) -> bool {
    match chunk.expr(eid) {
        Expr::Vararg => true,
        // Stop at function literals — their `...` is scoped to them.
        Expr::Function(_) => false,
        Expr::Index { obj, key } => {
            expr_uses_vararg(chunk, *obj) || expr_uses_vararg(chunk, *key)
        }
        Expr::Call { func, args, .. } => {
            expr_uses_vararg(chunk, *func) || args.iter().any(|&a| expr_uses_vararg(chunk, a))
        }
        Expr::MethodCall { obj, args, .. } => {
            expr_uses_vararg(chunk, *obj) || args.iter().any(|&a| expr_uses_vararg(chunk, a))
        }
        Expr::Table { fields, .. } => fields.iter().any(|f| table_field_uses_vararg(chunk, f)),
        Expr::BinOp { lhs, rhs, .. } => {
            expr_uses_vararg(chunk, *lhs) || expr_uses_vararg(chunk, *rhs)
        }
        Expr::UnOp { operand, .. } => expr_uses_vararg(chunk, *operand),
        Expr::Paren(inner) => expr_uses_vararg(chunk, *inner),
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Name(_) => false,
    }
}

fn table_field_uses_vararg(chunk: &Chunk, f: &TableField) -> bool {
    match f {
        TableField::Item(e) | TableField::Named(_, e) => expr_uses_vararg(chunk, *e),
        TableField::Keyed(k, v) => {
            expr_uses_vararg(chunk, *k) || expr_uses_vararg(chunk, *v)
        }
    }
}
