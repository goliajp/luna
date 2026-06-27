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
    block
        .stats
        .iter()
        .any(|&sid| stat_uses_vararg(chunk, chunk.stat(sid)))
}

fn stat_uses_vararg(chunk: &Chunk, stat: &Stat) -> bool {
    use Stat::*;
    match stat {
        Do(b) => block_uses_vararg(chunk, b),
        While { cond, body } => expr_uses_vararg(chunk, *cond) || block_uses_vararg(chunk, body),
        Repeat { body, cond } => block_uses_vararg(chunk, body) || expr_uses_vararg(chunk, *cond),
        If { arms, else_body } => {
            arms.iter()
                .any(|(c, _, b)| expr_uses_vararg(chunk, *c) || block_uses_vararg(chunk, b))
                || else_body
                    .as_ref()
                    .is_some_and(|b| block_uses_vararg(chunk, b))
        }
        NumericFor {
            start,
            limit,
            step,
            body,
            ..
        } => {
            expr_uses_vararg(chunk, *start)
                || expr_uses_vararg(chunk, *limit)
                || step.is_some_and(|s| expr_uses_vararg(chunk, s))
                || block_uses_vararg(chunk, body)
        }
        GenericFor { exprs, body, .. } => {
            exprs.iter().any(|&e| expr_uses_vararg(chunk, e)) || block_uses_vararg(chunk, body)
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
        Expr::Index { obj, key } => expr_uses_vararg(chunk, *obj) || expr_uses_vararg(chunk, *key),
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
        TableField::Keyed(k, v) => expr_uses_vararg(chunk, *k) || expr_uses_vararg(chunk, *v),
    }
}

// ---------------------------------------------------------------------------
// v2.1 Phase 11 — A4' prerequisite: RHS Call walker + metamethod-safety gate.
// ---------------------------------------------------------------------------
//
// A4' (RFC `v2.0-pi-phase11-a4-prime-rfc.md` §2) wants to skip the Index-LHS
// object snapshot Move at `compiler/mod.rs:2490` when the RHS of an assignment
// cannot re-bind the LHS local through a `__newindex` closure. The two helpers
// below model just the AST-side gate; the consumer (a future A4' attack) is
// expected to combine them with `LocalVar.captured` and target-arity checks.
//
// Pure additive in this batch: no current compile path calls these, so the
// only risk is dead-code warnings, which are silenced via #[allow] on the
// pub(crate) wrappers below until A4' wires them up.

/// Classification of call sites discovered in an RHS expression tree.
///
/// The walker partitions expressions into three buckets; an A4'-style gate
/// only accepts the bottom two (`None` and `OnlyKnownPure`) because
/// `UserOrUnknown` call sites can — through `__newindex` metamethod closure
/// capture — re-bind any local the closure has captured, which would
/// invalidate the Index-LHS snapshot elision.
///
/// This is intentionally an enum rather than a `bool`: a future relaxation
/// of the gate may distinguish between the two safe variants (e.g. to count
/// the third bucket for diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RhsCallScan {
    /// No `Call` or `MethodCall` AST nodes anywhere in the walked tree.
    /// Pure arith, literals, names, indices, paren and unary/binary chains.
    None,
    /// Calls present, but every callee is a `<stdlib_module>.<field>` shape
    /// (e.g. `math.min`, `string.byte`, `table.unpack`) where
    /// `<stdlib_module>` is a known-immutable stdlib root (see
    /// [`is_known_pure_stdlib_root`]). A known-pure call cannot run
    /// user-supplied Lua code, so it cannot trigger `__newindex` re-binding
    /// of an outer local.
    OnlyKnownPure,
    /// At least one `Call` or `MethodCall` site whose callee is not a
    /// known-pure stdlib lookup — could invoke user-defined Lua that
    /// re-binds locals via captured upvalues. A4' must reject this case.
    UserOrUnknown,
}

impl RhsCallScan {
    fn join(self, other: RhsCallScan) -> RhsCallScan {
        use RhsCallScan::*;
        match (self, other) {
            (UserOrUnknown, _) | (_, UserOrUnknown) => UserOrUnknown,
            (OnlyKnownPure, _) | (_, OnlyKnownPure) => OnlyKnownPure,
            (None, None) => None,
        }
    }
}

/// Stdlib module names whose top-level fields the gate treats as
/// known-pure (no user-Lua callback path through __index / __newindex).
///
/// CONSERVATIVE — the list is restricted to modules whose entries (in
/// luna's stdlib) are direct Rust builtins. `os` / `io` / `debug` /
/// `package` / `_G` / `_ENV` are excluded: they expose callbacks (`io.read`
/// can yield, `debug.sethook` invokes Lua, `os.exit` runs `__close`, etc.)
/// or are user-mutable.
///
/// Future-extensible: a Phase 11 attack may grow this list to include
/// `select` (top-level builtin), `tostring`, `tonumber`, etc. via a flat
/// names list. Left small intentionally for v2.1 ship.
fn is_known_pure_stdlib_root(text: &str) -> bool {
    matches!(text, "math" | "string" | "table")
}

/// Returns the [`RhsCallScan`] kind of the call sites reachable from `eid`.
///
/// Walks the AST tree rooted at `eid`, stopping at `Expr::Function` (its
/// body is its own scope and its `Call` ops fire only when the closure is
/// later invoked, not during RHS evaluation of the current statement).
///
/// O(n) in expression tree size — n is bounded by the source-character
/// count of the statement RHS. No allocation.
#[allow(dead_code)] // wired by the future A4' attack; pure additive in this batch.
pub(crate) fn walk_rhs_for_calls(chunk: &Chunk, eid: ExprId) -> RhsCallScan {
    use RhsCallScan::*;
    match chunk.expr(eid) {
        // Leaves — no calls.
        Expr::Nil
        | Expr::True
        | Expr::False
        | Expr::Vararg
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Name(_) => None,

        // Function literals don't *invoke* their body during RHS eval; the
        // value flowing out is the closure itself. (The closure may capture
        // upvalues but the capture happens at the `Closure` op, after the
        // snapshot site, and the captured local is rebound via the upvalue
        // *only* if the closure is later called — which the gate handles
        // by inspecting RHS Call ops, not Function literals.)
        Expr::Function(_) => None,

        Expr::Index { obj, key } => {
            walk_rhs_for_calls(chunk, *obj).join(walk_rhs_for_calls(chunk, *key))
        }
        Expr::Paren(inner) => walk_rhs_for_calls(chunk, *inner),
        Expr::UnOp { operand, .. } => walk_rhs_for_calls(chunk, *operand),
        Expr::BinOp { lhs, rhs, .. } => {
            walk_rhs_for_calls(chunk, *lhs).join(walk_rhs_for_calls(chunk, *rhs))
        }
        Expr::Table { fields, .. } => {
            let mut acc = None;
            for f in fields {
                let part = match f {
                    TableField::Item(e) | TableField::Named(_, e) => walk_rhs_for_calls(chunk, *e),
                    TableField::Keyed(k, v) => {
                        walk_rhs_for_calls(chunk, *k).join(walk_rhs_for_calls(chunk, *v))
                    }
                };
                acc = acc.join(part);
                if acc == UserOrUnknown {
                    return acc;
                }
            }
            acc
        }

        Expr::Call { func, args, .. } => {
            let here = classify_callee(chunk, *func);
            let mut acc = here;
            for &a in args {
                acc = acc.join(walk_rhs_for_calls(chunk, a));
                if acc == UserOrUnknown {
                    return acc;
                }
            }
            acc
        }
        Expr::MethodCall { obj, args, .. } => {
            // `obj:method(args)` is morally `obj.method(obj, args)`. Even if
            // `obj` is a known-pure stdlib root the *method dispatch* itself
            // may hit a __index path, so MethodCall is unconditionally
            // UserOrUnknown for the gate. Conservative; can be relaxed
            // later if obj is a literal stdlib lookup.
            let mut acc = UserOrUnknown;
            // Still walk for diagnostics / future relaxation, but the result
            // can only go up from UserOrUnknown.
            acc = acc.join(walk_rhs_for_calls(chunk, *obj));
            for &a in args {
                acc = acc.join(walk_rhs_for_calls(chunk, a));
            }
            acc
        }
    }
}

/// Classifies the callee of a `Call` node in isolation (does NOT recurse
/// into the args, which is the caller's job).
fn classify_callee(chunk: &Chunk, callee: ExprId) -> RhsCallScan {
    match chunk.expr(callee) {
        // `math.min(...)` shape: callee is Index{ Name(known_root), Str(field) }.
        Expr::Index { obj, key } => {
            let root_ok = matches!(
                chunk.expr(*obj),
                Expr::Name(n) if is_known_pure_stdlib_root(&n.text)
            );
            let key_is_str = matches!(chunk.expr(*key), Expr::Str(_));
            if root_ok && key_is_str {
                RhsCallScan::OnlyKnownPure
            } else {
                RhsCallScan::UserOrUnknown
            }
        }
        // Bare name callee (`f(...)`) or anything else: unknown.
        // `_ENV.math.min` and similar dotted globals do NOT match — keep
        // the gate strict, the consumer can opt in later.
        _ => RhsCallScan::UserOrUnknown,
    }
}

/// Metamethod-safety gate for the Index-LHS snapshot elision attack
/// described in `.dev/rfcs/v2.0-pi-phase11-a4-prime-rfc.md` §2.
///
/// Returns `true` only when, based purely on AST shape:
///
/// 1. `obj_eid` is a bare local-name reference (`Expr::Name`). A4'
///    requires the LHS object to be a stable, non-captured local. The
///    consumer is still expected to verify `captured == false` against
///    `LocalVar` at the call site — this gate only handles the AST half.
/// 2. `rhs_eid`'s call sites are all classified as
///    [`RhsCallScan::None`] or [`RhsCallScan::OnlyKnownPure`].
///
/// Returns `false` whenever the gate cannot prove safety (closed-world
/// pessimism — unknown = unsafe).
///
/// ## Conservative gaps (intentional, deferred)
///
/// - Closure-capture modeling is NOT performed. Even an
///   `OnlyKnownPure` RHS could in principle re-bind via a closure
///   stored on the metatable, but luna's stdlib does not call back
///   into Lua, so the gate is sound for the v2.1 ship surface.
/// - `_ENV.math.min` (dotted global through the env upvalue) is treated
///   as unsafe even though it ultimately resolves to the same builtin.
/// - Local `f` aliased to `math.min` (e.g. `local m = math.min; m(x)`)
///   is treated as unsafe. Variable-tracking is a separate subsystem.
/// - `obj` that is itself an Index (e.g. `t.a.b = v`) is rejected —
///   only direct local Index-LHS is in scope for A4' v1.
#[allow(dead_code)] // wired by the future A4' attack; pure additive in this batch.
pub(crate) fn metamethod_safe_for_index_lhs(
    chunk: &Chunk,
    obj_eid: ExprId,
    rhs_eid: ExprId,
) -> bool {
    if !matches!(chunk.expr(obj_eid), Expr::Name(_)) {
        return false;
    }
    matches!(
        walk_rhs_for_calls(chunk, rhs_eid),
        RhsCallScan::None | RhsCallScan::OnlyKnownPure
    )
}
