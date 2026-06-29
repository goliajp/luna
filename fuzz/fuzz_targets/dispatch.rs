//! v2.4 Phase Fuzz-B — dispatcher fuzz target.
//!
//! Random bytes → constrained-grammar Lua AST (via the `Arbitrary`
//! derive) → text source → `Vm::eval`. Asserts:
//! - no panic in dispatcher / GC / builtin paths
//! - no UB / OOB (ASAN runtime catches these)
//!
//! The constrained grammar always renders to valid 5.5 Lua so the
//! parser succeeds and the bytecode actually executes (vs the
//! `parse` target which mostly hits parse-error paths). Bound:
//! every program runs against a memory cap so a generator that
//! happens to write `for i = 1, math.huge do t[i] = i end` trips
//! the cap cleanly instead of OOMing the runner.
//!
//! Run locally:
//!     cd fuzz && cargo +nightly fuzz run dispatch \
//!         --target aarch64-apple-darwin -- -runs=10000
//!
//! Nightly CI: `.github/workflows/fuzz.yml` (Phase Fuzz-E).

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::fmt::Write;

/// Constrained Lua expression that always renders parseable.
#[derive(Arbitrary, Debug)]
enum Expr {
    Int(i32),
    Float(f32),
    Nil,
    True,
    False,
    /// reference to one of the predeclared local variables `a`..`e`
    VarRef(VarIdx),
    BinOp(Box<Expr>, BinOp, Box<Expr>),
    UnOp(UnOp, Box<Expr>),
    /// `string.format("%d", expr)` — exercises string + format paths
    Format(Box<Expr>),
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum BinOp {
    Add,
    Sub,
    Mul,
    Mod,
    Lt,
    Le,
    Eq,
    And,
    Or,
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum UnOp {
    Neg,
    Not,
}

/// One of 5 predeclared local variables (a, b, c, d, e). Bounded to
/// keep the rendered source small + dispatcher reachable.
#[derive(Arbitrary, Debug, Clone, Copy)]
enum VarIdx {
    A,
    B,
    C,
    D,
    E,
}

/// Single statement. Limited to safe shapes — no metatable / userdata
/// / coroutine constructs at this fuzz level (those need their own
/// targeted seed corpus later).
#[derive(Arbitrary, Debug)]
enum Stmt {
    Assign(VarIdx, Expr),
    /// `if expr then <stmt>; end`
    If(Expr, Box<Stmt>),
    /// `while expr do <stmt>; end` — bounded by an outer instruction
    /// budget so infinite-loop generators don't hang the fuzzer.
    While(Expr, Box<Stmt>),
    /// `for i = 1, expr do <stmt>; end`
    For(Expr, Box<Stmt>),
    /// `<expr>` evaluated for side-effect — most exprs are pure so
    /// no observable effect, but it exercises the discard path.
    DiscardExpr(Expr),
}

/// Top-level program: a list of statements, bounded to keep
/// generated source linear in input bytes.
#[derive(Arbitrary, Debug)]
struct Program {
    stmts: Vec<Stmt>,
}

impl VarIdx {
    fn name(self) -> &'static str {
        match self {
            VarIdx::A => "a",
            VarIdx::B => "b",
            VarIdx::C => "c",
            VarIdx::D => "d",
            VarIdx::E => "e",
        }
    }
}

impl BinOp {
    fn lua(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Mod => "%",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Eq => "==",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }
}

fn render_expr(buf: &mut String, e: &Expr, depth: u32) {
    if depth > 6 {
        // Bound recursion in renderer to avoid huge sources.
        buf.push_str("0");
        return;
    }
    match e {
        Expr::Int(i) => write!(buf, "({i})").unwrap(),
        Expr::Float(f) => write!(buf, "({})", f).unwrap(),
        Expr::Nil => buf.push_str("nil"),
        Expr::True => buf.push_str("true"),
        Expr::False => buf.push_str("false"),
        Expr::VarRef(v) => buf.push_str(v.name()),
        Expr::BinOp(l, op, r) => {
            buf.push('(');
            render_expr(buf, l, depth + 1);
            write!(buf, " {} ", op.lua()).unwrap();
            render_expr(buf, r, depth + 1);
            buf.push(')');
        }
        Expr::UnOp(UnOp::Neg, x) => {
            buf.push_str("(-");
            render_expr(buf, x, depth + 1);
            buf.push(')');
        }
        Expr::UnOp(UnOp::Not, x) => {
            buf.push_str("(not ");
            render_expr(buf, x, depth + 1);
            buf.push(')');
        }
        Expr::Format(x) => {
            buf.push_str("string.format(\"%s\", tostring(");
            render_expr(buf, x, depth + 1);
            buf.push_str("))");
        }
    }
}

fn render_stmt(buf: &mut String, s: &Stmt, depth: u32) {
    if depth > 5 {
        // Bound block nesting; deeper nests collapse to a no-op.
        buf.push_str("\n");
        return;
    }
    match s {
        Stmt::Assign(v, e) => {
            write!(buf, "{} = ", v.name()).unwrap();
            render_expr(buf, e, 0);
            buf.push('\n');
        }
        Stmt::If(cond, body) => {
            buf.push_str("if ");
            render_expr(buf, cond, 0);
            buf.push_str(" then\n");
            render_stmt(buf, body, depth + 1);
            buf.push_str("end\n");
        }
        Stmt::While(cond, body) => {
            // Bound iteration count to keep the fuzzer's per-input
            // budget under control — a `while true do nothing end`
            // would consume the dispatch budget alone.
            buf.push_str("do local __i=0; while ");
            render_expr(buf, cond, 0);
            buf.push_str(" and __i<32 do __i=__i+1\n");
            render_stmt(buf, body, depth + 1);
            buf.push_str("end end\n");
        }
        Stmt::For(end, body) => {
            buf.push_str("for __k = 1, math.min(32, math.max(1, math.abs(");
            render_expr(buf, end, 0);
            buf.push_str("))) do\n");
            render_stmt(buf, body, depth + 1);
            buf.push_str("end\n");
        }
        Stmt::DiscardExpr(e) => {
            buf.push_str("local _ = ");
            render_expr(buf, e, 0);
            buf.push('\n');
        }
    }
}

fn render_program(p: &Program) -> String {
    // Bound program size: take at most 32 statements per input.
    let mut buf = String::new();
    // Predeclare locals so VarRef can be valid from the first stmt.
    buf.push_str("local a, b, c, d, e = 1, 2, 3, 4, 5\n");
    for s in p.stmts.iter().take(32) {
        render_stmt(&mut buf, s, 0);
    }
    buf
}

fuzz_target!(|p: Program| {
    let source = render_program(&p);
    let mut vm = Vm::new(LuaVersion::Lua55);
    // 16 MiB memory cap — generous enough for normal programs,
    // tight enough to catch a runaway-allocation generator
    // cleanly via the catchable `"memory cap exceeded"` Lua error
    // (which `Vm::eval` returns as `Err`, NOT a panic).
    vm.set_memory_cap(Some(16 * 1024 * 1024));
    let _ = vm.eval(&source);
});
