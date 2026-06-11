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
        /// `(condition, body)` for the `if` and each `elseif`.
        arms: Vec<(ExprId, Block)>,
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
    pub block: Block,
}

impl Chunk {
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }

    pub fn stat(&self, id: StatId) -> &Stat {
        &self.stats[id.0 as usize]
    }
}
