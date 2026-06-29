//! v2.4 Phase Fuzz-D — differential vs PUC fuzz target.
//!
//! Arbitrary-derived side-effect-free Expr → list of `print(expr)`
//! stmts → byte-diff luna vs PUC reference binary (`$PUC_LUA`,
//! default `lua5.5`). Panics on any divergence — that's a luna bug.
//!
//! Skipped (no panic) when `$PUC_LUA` is unset OR the PUC binary
//! fails to spawn — local dev without PUC installed shouldn't fail
//! the fuzz harness; CI's `.github/workflows/fuzz.yml` installs
//! lua5.5 explicitly when matrix target = fuzz_diff_puc.
//!
//! Why this complements the static `diff_puc.rs` integration test:
//! that test ships 5 hand-picked deterministic fixtures. This fuzz
//! target generates infinite distinct programs + catches
//! semantic-divergence bugs the fixed corpus misses.
//!
//! Run:
//!     PUC_LUA=$(which lua5.5) cd crates/luna-fuzz
//!     cargo +nightly fuzz run fuzz_diff_puc -- -runs=1000

#![no_main]

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::process::{Command, Stdio};

#[derive(Arbitrary, Debug)]
enum Expr {
    Int(i32),
    Float(NormalFloat),
    Nil,
    True,
    False,
    Var(VarIdx),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Mod(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
}

/// Floats restricted to a narrow non-pathological range — PUC's
/// `tostring` formatting of NaN / Inf / very-small / very-large
/// floats has corner-case spelling drift vs luna; v2.5+ tightens.
#[derive(Debug)]
struct NormalFloat(f64);

impl<'a> Arbitrary<'a> for NormalFloat {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let n: i32 = u.arbitrary()?;
        let f = ((n as f64) / 1000.0).clamp(-100.0, 100.0);
        Ok(NormalFloat(f))
    }
}

#[derive(Arbitrary, Debug, Clone, Copy)]
enum VarIdx {
    A,
    B,
    C,
}

impl VarIdx {
    fn name(self) -> &'static str {
        match self {
            VarIdx::A => "a",
            VarIdx::B => "b",
            VarIdx::C => "c",
        }
    }
}

#[derive(Arbitrary, Debug)]
struct Program {
    prints: Vec<Expr>,
}

fn render_expr(buf: &mut String, e: &Expr, depth: u32) {
    if depth > 4 {
        buf.push('0');
        return;
    }
    match e {
        Expr::Int(i) => write!(buf, "({i})").unwrap(),
        Expr::Float(NormalFloat(f)) => write!(buf, "({})", f).unwrap(),
        Expr::Nil => buf.push_str("nil"),
        Expr::True => buf.push_str("true"),
        Expr::False => buf.push_str("false"),
        Expr::Var(v) => buf.push_str(v.name()),
        Expr::Add(l, r) => bin(buf, "+", l, r, depth),
        Expr::Sub(l, r) => bin(buf, "-", l, r, depth),
        Expr::Mul(l, r) => bin(buf, "*", l, r, depth),
        Expr::Mod(l, r) => {
            // Guard divisor != 0 to avoid luna-vs-PUC error-message
            // wording drift.
            buf.push('(');
            render_expr(buf, l, depth + 1);
            buf.push_str(" % ((");
            render_expr(buf, r, depth + 1);
            buf.push_str(") ~= 0 and (");
            render_expr(buf, r, depth + 1);
            buf.push_str(") or 1))");
        }
        Expr::Lt(l, r) => bin(buf, "<", l, r, depth),
    }
}

fn bin(buf: &mut String, op: &str, l: &Expr, r: &Expr, depth: u32) {
    buf.push('(');
    render_expr(buf, l, depth + 1);
    write!(buf, " {} ", op).unwrap();
    render_expr(buf, r, depth + 1);
    buf.push(')');
}

fn render(p: &Program) -> String {
    let mut buf = String::from("local a, b, c = 1, 2, 3\n");
    for e in p.prints.iter().take(16) {
        buf.push_str("print(");
        render_expr(&mut buf, e, 0);
        buf.push_str(")\n");
    }
    buf
}

fn run_puc(source: &str) -> Option<String> {
    let bin = std::env::var("PUC_LUA").ok()?;
    let mut child = Command::new(&bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(source.as_bytes());
    }
    let out = child.wait_with_output().ok()?;
    if !out.status.success() || !out.stderr.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run_luna(source: &str) -> Option<String> {
    const PREAMBLE: &str = r#"
_G.__diff_puc_buf = ""
function print(...)
    local t = {}
    local n = select('#', ...)
    for i = 1, n do t[i] = tostring(select(i, ...)) end
    _G.__diff_puc_buf = _G.__diff_puc_buf .. table.concat(t, '\t') .. '\n'
end
"#;
    let mut full = String::with_capacity(PREAMBLE.len() + source.len() + 32);
    full.push_str(PREAMBLE);
    full.push_str(source);
    full.push_str("\nreturn _G.__diff_puc_buf\n");
    let mut vm = Vm::new(LuaVersion::Lua55);
    vm.set_memory_cap(Some(16 * 1024 * 1024));
    let r = vm.eval(&full).ok()?;
    match r.first() {
        Some(Value::Str(s)) => Some(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        _ => None,
    }
}

fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}

fuzz_target!(|p: Program| {
    let source = render(&p);
    let Some(puc) = run_puc(&source) else { return };
    let Some(luna) = run_luna(&source) else {
        panic!(
            "luna eval failed where PUC succeeded\n=== source ===\n{source}\n=== PUC stdout ===\n{puc}"
        );
    };
    let puc_n = normalize(&puc);
    let luna_n = normalize(&luna);
    if puc_n != luna_n {
        panic!(
            "diff_puc: luna ≠ PUC\n=== source ===\n{source}\n=== PUC ===\n{puc_n}\n=== luna ===\n{luna_n}"
        );
    }
});
