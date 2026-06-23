//! Arena AST: nodes live in flat vectors inside [`Chunk`], referenced by
//! typed 4-byte ids. Dense storage, no per-node boxing.

/// Typed index into [`Chunk::exprs`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExprId(
    /// Zero-based offset into the chunk's expression arena.
    pub u32,
);

/// Typed index into [`Chunk::stats`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatId(
    /// Zero-based offset into the chunk's statement arena.
    pub u32,
);

/// An identifier token captured during parsing, together with its source
/// line for error reporting and debug-info emission.
#[derive(Clone, Debug)]
pub struct Name {
    /// UTF-8 source text of the identifier.
    pub text: Box<str>,
    /// 1-based source line where the identifier was lexed.
    pub line: u32,
}

/// Lua 5.4+ local-variable attribute (`<const>` / `<close>`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attrib {
    /// `<const>` — immutable local binding.
    Const,
    /// `<close>` — to-be-closed local; closes on scope exit (5.4).
    Close,
}

/// One declared name with its optional `<attrib>`.
#[derive(Clone, Debug)]
pub struct AttribName {
    /// Identifier being declared.
    pub name: Name,
    /// Optional attribute (`<const>` / `<close>`).
    pub attrib: Option<Attrib>,
}

/// A sequence of statements; the lexical scope unit in Lua.
#[derive(Clone, Debug)]
pub struct Block {
    /// Statements in source order.
    pub stats: Vec<StatId>,
}

/// `function a.b.c:m() ...` target path.
#[derive(Clone, Debug)]
pub struct FuncName {
    /// First identifier in the path (`a` in `a.b.c:m`).
    pub base: Name,
    /// Dotted sub-keys after the base, in left-to-right order.
    pub path: Vec<Name>,
    /// Method name after `:`, if any (adds an implicit `self` parameter).
    pub method: Option<Name>,
}

/// Vararg form for a function definition.
#[derive(Clone, Debug)]
pub enum Vararg {
    /// No vararg in the parameter list.
    None,
    /// Anonymous `...`; accessible via `...` in the body.
    Anonymous,
    /// 5.5 named vararg table: `function f(...t)`.
    Named(
        /// Bound name receiving the captured varargs as a sequence.
        Name,
    ),
}

/// A function literal's body — parameters plus the contained block.
#[derive(Clone, Debug)]
pub struct FuncBody {
    /// Fixed parameter list, in declaration order.
    pub params: Vec<Name>,
    /// Vararg form, if any.
    pub vararg: Vararg,
    /// Body block.
    pub block: Block,
    /// Source line of the opening `function` / `(` token.
    pub line: u32,
    /// line of the closing `end` (PUC `lastlinedefined`)
    pub end_line: u32,
}

/// Top-level statement kinds — every Lua syntactic form except expressions.
#[derive(Clone, Debug)]
pub enum Stat {
    /// `do ... end` block.
    Do(
        /// Inner block.
        Block,
    ),
    /// `while cond do ... end`.
    While {
        /// Loop condition evaluated each iteration.
        cond: ExprId,
        /// Loop body.
        body: Block,
    },
    /// `repeat ... until cond`.
    Repeat {
        /// Loop body executed before testing.
        body: Block,
        /// Termination condition.
        cond: ExprId,
    },
    /// `if ... elseif ... else ... end`.
    If {
        /// `(condition, then_line, body)` for the `if` and each `elseif`. The
        /// `then_line` is the source line of the `then` keyword for that arm
        /// — PUC 5.3 attributes the conditional-skip JMP to that line so a
        /// taken if-then-else fires a line hook for the `then` keyword before
        /// the body (`for i=1,n do … then … end` traces include the `then`
        /// keyword line). 5.4 collapsed that back to the body's first line;
        /// see `if_stat` in the compiler for the version split.
        arms: Vec<(ExprId, u32, Block)>,
        /// Optional `else` body.
        else_body: Option<Block>,
    },
    /// `for var = start, limit [, step] do ... end`.
    NumericFor {
        /// Induction variable.
        var: Name,
        /// Starting value expression.
        start: ExprId,
        /// Upper bound expression.
        limit: ExprId,
        /// Optional step expression (defaults to `1`).
        step: Option<ExprId>,
        /// Loop body.
        body: Block,
    },
    /// `for v1, v2, ... in exprs do ... end`.
    GenericFor {
        /// Loop variables receiving each iterator call's results.
        vars: Vec<Name>,
        /// Expression list yielding iterator, state, control, and (5.4)
        /// to-be-closed value.
        exprs: Vec<ExprId>,
        /// Loop body.
        body: Block,
        /// Line of the first token after `in` (PUC `forlist` `line`); used to
        /// attribute the per-iteration `TFORCALL` so a non-callable iterator
        /// (`for k,v in 3 do …`) raises on the EXPR's source line, not the
        /// `for` line.
        expr_line: u32,
    },
    /// `local [<attrib>] names = exprs`.
    Local {
        /// Single attribute applied to every name (5.4 `local <const>`).
        collective: Option<Attrib>,
        /// Names being introduced, each with its optional per-name attribute.
        names: Vec<AttribName>,
        /// Initializer expressions; missing names get `nil`.
        exprs: Vec<ExprId>,
    },
    /// 5.5 `global` declaration.
    Global {
        /// Attribute applied to every name.
        collective: Option<Attrib>,
        /// Declared global names.
        names: Vec<AttribName>,
        /// Initializer expressions.
        exprs: Vec<ExprId>,
    },
    /// 5.5 `global [attrib] *`.
    GlobalAll {
        /// Attribute applied to all subsequently introduced globals.
        attrib: Option<Attrib>,
    },
    /// Multiple assignment `targets = exprs`.
    Assign {
        /// Assignment targets — each must be an lvalue (`Name` / `Index`).
        targets: Vec<ExprId>,
        /// Right-hand side expressions, evaluated before any target is
        /// assigned.
        exprs: Vec<ExprId>,
    },
    /// Expression statement (function or method call).
    Call(
        /// The call expression.
        ExprId,
    ),
    /// `function a.b.c:m() ... end`.
    Function {
        /// Target path of the assignment.
        name: FuncName,
        /// Function body.
        body: FuncBody,
    },
    /// `local function name() ... end`.
    LocalFunction {
        /// Local name being bound.
        name: Name,
        /// Function body.
        body: FuncBody,
    },
    /// 5.5 `global function f() ...`.
    GlobalFunction {
        /// Global name being bound.
        name: Name,
        /// Function body.
        body: FuncBody,
    },
    /// `return exprs`.
    Return {
        /// Returned expressions; empty for a bare `return`.
        exprs: Vec<ExprId>,
        /// Source line of the `return` keyword.
        line: u32,
    },
    /// `break`.
    Break {
        /// Source line of the `break` keyword.
        line: u32,
    },
    /// `goto label`.
    Goto(
        /// Target label.
        Name,
    ),
    /// `::label::` declaration.
    Label(
        /// Label name.
        Name,
    ),
}

/// Binary operator kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    /// `+` arithmetic addition.
    Add,
    /// `-` arithmetic subtraction.
    Sub,
    /// `*` arithmetic multiplication.
    Mul,
    /// `/` float division (always returns float).
    Div,
    /// `//` floor division.
    IDiv,
    /// `%` modulo.
    Mod,
    /// `^` exponentiation (always returns float).
    Pow,
    /// `..` string concatenation.
    Concat,
    /// `==` equality.
    Eq,
    /// `~=` inequality.
    Ne,
    /// `<` less than.
    Lt,
    /// `<=` less than or equal.
    Le,
    /// `>` greater than.
    Gt,
    /// `>=` greater than or equal.
    Ge,
    /// `and` short-circuiting conjunction.
    And,
    /// `or` short-circuiting disjunction.
    Or,
    /// `&` bitwise AND.
    BAnd,
    /// `|` bitwise OR.
    BOr,
    /// `~` bitwise XOR.
    BXor,
    /// `<<` left shift.
    Shl,
    /// `>>` right shift.
    Shr,
}

/// Unary operator kinds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UnOp {
    /// `-` arithmetic negation.
    Neg,
    /// `not` logical negation.
    Not,
    /// `#` length operator.
    Len,
    /// `~` bitwise NOT.
    BNot,
}

/// One field in a table constructor literal.
#[derive(Clone, Debug)]
pub enum TableField {
    /// positional `expr`
    Item(
        /// Value expression.
        ExprId,
    ),
    /// `name = expr`
    Named(
        /// Field name used as a string key.
        Name,
        /// Value expression.
        ExprId,
    ),
    /// `[key] = expr`
    Keyed(
        /// Key expression.
        ExprId,
        /// Value expression.
        ExprId,
    ),
}

/// Expression kinds — produces a Lua value when evaluated.
#[derive(Clone, Debug)]
pub enum Expr {
    /// `nil` literal.
    Nil,
    /// `true` literal.
    True,
    /// `false` literal.
    False,
    /// `...` vararg expression (legal only inside a vararg function).
    Vararg,
    /// Integer literal.
    Int(
        /// The 64-bit signed integer value.
        i64,
    ),
    /// Floating-point literal.
    Float(
        /// The IEEE-754 double value.
        f64,
    ),
    /// String literal (raw bytes — Lua strings are 8-bit clean).
    Str(
        /// Raw byte contents (no terminator).
        Vec<u8>,
    ),
    /// Identifier reference (resolved later to local / upvalue / global).
    Name(
        /// The identifier.
        Name,
    ),
    /// `obj.key` and `obj[key]` (dot keys become string-literal keys).
    Index {
        /// Container expression.
        obj: ExprId,
        /// Key expression.
        key: ExprId,
    },
    /// `func(args)` function call.
    Call {
        /// Callee expression.
        func: ExprId,
        /// Argument expressions in call order.
        args: Vec<ExprId>,
        /// Source line of the call site.
        line: u32,
    },
    /// `obj:method(args)` method call (passes `obj` as implicit first arg).
    MethodCall {
        /// Receiver expression.
        obj: ExprId,
        /// Method name (looked up on `obj`).
        method: Name,
        /// Argument expressions after the implicit receiver.
        args: Vec<ExprId>,
        /// Source line of the call site.
        line: u32,
    },
    /// `function ... end` function literal.
    Function(
        /// Function body.
        FuncBody,
    ),
    /// `{ ... }` table constructor.
    Table {
        /// Fields in source order.
        fields: Vec<TableField>,
        /// Source line of the opening `{`.
        line: u32,
    },
    /// Binary operator expression.
    BinOp {
        /// Operator.
        op: BinOp,
        /// Left operand.
        lhs: ExprId,
        /// Right operand.
        rhs: ExprId,
        /// Source line for error reporting.
        line: u32,
    },
    /// Unary operator expression.
    UnOp {
        /// Operator.
        op: UnOp,
        /// Operand.
        operand: ExprId,
        /// Source line for error reporting.
        line: u32,
    },
    /// Parenthesized expression: truncates multiple results to one.
    Paren(
        /// Inner expression.
        ExprId,
    ),
}

/// A parsed chunk: the top-level block plus the node arenas.
#[derive(Clone, Debug)]
pub struct Chunk {
    /// Arena of all expression nodes; index with [`ExprId`].
    pub exprs: Vec<Expr>,
    /// Arena of all statement nodes; index with [`StatId`].
    pub stats: Vec<Stat>,
    /// starting source line of each statement, indexed by `StatId`
    pub stat_lines: Vec<u32>,
    /// Top-level block (the script body).
    pub block: Block,
    /// line of the final `<eof>` token (PUC main-chunk `lastlinedefined`); the
    /// implicit final return is attributed here
    pub end_line: u32,
}

impl Chunk {
    /// Borrow an expression node by id.
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }

    /// Borrow a statement node by id.
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
