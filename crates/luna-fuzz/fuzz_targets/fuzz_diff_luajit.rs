//! v2.6 Track C — differential vs LuaJIT fuzz target.
//!
//! Arbitrary-derived side-effect-free Expr → list of `print(expr)`
//! stmts → byte-diff luna vs LuaJIT reference binary (`$LUAJIT`,
//! default `luajit`). Panics on any divergence — that's a luna
//! bug OR a known luna-vs-LuaJIT semantic gap (filed as issue,
//! grammar narrowed in next pass).
//!
//! Skipped (no panic) when `$LUAJIT` is unset OR the LuaJIT
//! binary fails to spawn — local dev without LuaJIT installed
//! shouldn't fail the fuzz harness; CI's
//! `.github/workflows/diff-luajit.yml` installs LuaJIT
//! explicitly.
//!
//! Why this complements fuzz_diff_puc:
//! LuaJIT diverges from PUC in several documented places
//! (integer arithmetic semantics, certain tostring formatting,
//! `string.format` edge cases). luna currently aligns with PUC
//! 5.4/5.5; this target surfaces unintentional LuaJIT
//! divergences that may matter for the embedder ecosystem
//! (kevy, Redis-replica targets).
//!
//! Run:
//!     LUAJIT=$(which luajit) cd crates/luna-fuzz
//!     cargo +nightly fuzz run --fuzz-dir . fuzz_diff_luajit \
//!         -- -runs=1000

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

/// Floats restricted to a narrow non-pathological range — both
/// engines disagree on `tostring` of NaN / Inf / very-small /
/// very-large floats. Narrower than fuzz_diff_puc because LuaJIT
/// adds its own formatting drift on top of PUC's.
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
        // LuaJIT prints ints/floats with subtly different
        // formatting than PUC; wrap in tostring(math.floor(...))
        // so the comparison is on integer string only.
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
        // tostring-wrap each print so float vs int formatting
        // drift between LuaJIT and luna doesn't dominate
        // divergence noise.
        buf.push_str("print(tostring(");
        render_expr(&mut buf, e, 0);
        buf.push_str("))\n");
    }
    buf
}

fn run_luajit(source: &str) -> Option<String> {
    let bin = std::env::var("LUAJIT").ok()?;
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
_G.__diff_luajit_buf = ""
function print(...)
    local t = {}
    local n = select('#', ...)
    for i = 1, n do t[i] = tostring(select(i, ...)) end
    _G.__diff_luajit_buf = _G.__diff_luajit_buf .. table.concat(t, '\t') .. '\n'
end
"#;
    let mut full = String::with_capacity(PREAMBLE.len() + source.len() + 32);
    full.push_str(PREAMBLE);
    full.push_str(source);
    full.push_str("\nreturn _G.__diff_luajit_buf\n");
    // LuaJIT defaults to Lua 5.1 syntax + LuaJIT extensions;
    // luna's Lua51 dialect is the closest match.
    let mut vm = Vm::new(LuaVersion::Lua51);
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
    let Some(luajit) = run_luajit(&source) else {
        return;
    };
    let Some(luna) = run_luna(&source) else {
        // LuaJIT succeeded but luna failed — file as divergence.
        panic!(
            "luna eval failed where LuaJIT succeeded\n=== source ===\n{source}\n=== LuaJIT stdout ===\n{luajit}"
        );
    };
    let luajit_n = normalize(&luajit);
    let luna_n = normalize(&luna);
    if luajit_n != luna_n {
        panic!(
            "diff_luajit: luna ≠ LuaJIT\n=== source ===\n{source}\n=== LuaJIT ===\n{luajit_n}\n=== luna ===\n{luna_n}"
        );
    }
});
