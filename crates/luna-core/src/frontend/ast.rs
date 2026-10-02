//! Arena AST: nodes live in flat vectors inside [`Chunk`], referenced by
//! typed 4-byte ids. Lists of ids (a block's statements, a call's
//! arguments, ...) are ranges into shared vectors of the chunk ([`List`]),
//! and names and string literals are numbers into the chunk's [`Names`].
//! Building a tree allocates no memory per node.

mod list;
mod rhs_calls;
mod vararg;
pub use super::names::{Names, Sym};
pub use list::{List, ListItem};
pub use rhs_calls::*;
pub use vararg::block_uses_vararg;

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

/// An identifier captured during parsing, together with its source line
/// for error reporting and debug-info emission. Its text is
/// [`Chunk::name`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Name {
    /// The identifier's number in [`Chunk::names`].
    pub sym: Sym,
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
#[derive(Clone, Copy, Debug)]
pub struct AttribName {
    /// Identifier being declared.
    pub name: Name,
    /// Optional attribute (`<const>` / `<close>`).
    pub attrib: Option<Attrib>,
}

/// A sequence of statements; the lexical scope unit in Lua.
#[derive(Clone, Copy, Debug)]
pub struct Block {
    /// Statements in source order.
    pub stats: List<StatId>,
}

/// One `if` / `elseif` arm of [`Stat::If`].
#[derive(Clone, Copy, Debug)]
pub struct IfArm {
    /// The arm's condition.
    pub cond: ExprId,
    /// Source line of the arm's `then` keyword. PUC 5.3 attributes the
    /// conditional-skip JMP to that line so a taken if-then-else fires a
    /// line hook for the `then` keyword before the body; 5.4 collapsed that
    /// back to the body's first line.
    pub then_line: u32,
    /// The arm's body.
    pub body: Block,
}

/// `function a.b.c:m() ...` target path.
#[derive(Clone, Copy, Debug)]
pub struct FuncName {
    /// First identifier in the path (`a` in `a.b.c:m`).
    pub base: Name,
    /// Dotted sub-keys after the base, in left-to-right order.
    pub path: List<Name>,
    /// Method name after `:`, if any (adds an implicit `self` parameter).
    pub method: Option<Name>,
}

/// Vararg form for a function definition.
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug)]
pub struct FuncBody {
    /// Fixed parameter list, in declaration order.
    pub params: List<Name>,
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
#[derive(Clone, Copy, Debug)]
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
        /// The `if` arm and each `elseif` arm, in source order.
        arms: List<IfArm>,
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
        vars: List<Name>,
        /// Expression list yielding iterator, state, control, and (5.4)
        /// to-be-closed value.
        exprs: List<ExprId>,
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
        names: List<AttribName>,
        /// Initializer expressions; missing names get `nil`.
        exprs: List<ExprId>,
    },
    /// 5.5 `global` declaration.
    Global {
        /// Attribute applied to every name.
        collective: Option<Attrib>,
        /// Declared global names.
        names: List<AttribName>,
        /// Initializer expressions.
        exprs: List<ExprId>,
    },
    /// 5.5 `global [attrib] *`.
    GlobalAll {
        /// Attribute applied to all subsequently introduced globals.
        attrib: Option<Attrib>,
    },
    /// Multiple assignment `targets = exprs`.
    Assign {
        /// Assignment targets — each must be an lvalue (`Name` / `Index`).
        targets: List<ExprId>,
        /// Right-hand side expressions, evaluated before any target is
        /// assigned.
        exprs: List<ExprId>,
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
        exprs: List<ExprId>,
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
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy, Debug)]
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
    /// String literal (raw bytes — Lua strings are 8-bit clean); also the
    /// key of `obj.name`.
    Str(
        /// The literal's number in [`Chunk::names`]; its bytes are
        /// [`Chunk::str`].
        Sym,
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
        args: List<ExprId>,
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
        args: List<ExprId>,
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
        fields: List<TableField>,
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

/// A parsed chunk: the top-level block plus the node arenas, the list
/// vectors the nodes' [`List`]s point into, and the chunk's names.
///
/// Walk it from [`Chunk::block`]: [`Chunk::stat`] and [`Chunk::expr`] give
/// the nodes, [`Chunk::list`] the ids and items of a [`List`], and
/// [`Chunk::name`] / [`Chunk::str`] the text of a [`Name`] or literal.
#[derive(Clone, Debug, Default)]
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
    /// The identifiers and string literals the nodes refer to.
    pub names: Names,
    /// Items of every `List<ExprId>`.
    pub expr_lists: Vec<ExprId>,
    /// Items of every `List<StatId>` (block bodies).
    pub stat_lists: Vec<StatId>,
    /// Items of every `List<Name>`.
    pub name_lists: Vec<Name>,
    /// Items of every `List<AttribName>`.
    pub attrib_name_lists: Vec<AttribName>,
    /// Items of every `List<TableField>`.
    pub field_lists: Vec<TableField>,
    /// Items of every `List<IfArm>`.
    pub arm_lists: Vec<IfArm>,
}

impl Default for Block {
    fn default() -> Block {
        Block { stats: List::EMPTY }
    }
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

    /// The items of a list.
    pub fn list<T: ListItem>(&self, l: List<T>) -> &[T] {
        &T::items(self)[l.range()]
    }

    /// The statements of a block, in source order.
    pub fn block_stats(&self, b: &Block) -> &[StatId] {
        self.list(b.stats)
    }

    /// The text of an identifier.
    pub fn name(&self, n: Name) -> &str {
        self.names.text(n.sym)
    }

    /// The bytes of a string literal (or of any entry of [`Chunk::names`]).
    pub fn str(&self, s: Sym) -> &[u8] {
        self.names.bytes(s)
    }

    /// Store `items` as a new list.
    pub fn push_list<T: ListItem>(&mut self, items: &[T]) -> List<T> {
        let v = T::items_mut(self);
        let start = v.len() as u32;
        v.extend_from_slice(items);
        List::new(start, items.len() as u32)
    }
}
