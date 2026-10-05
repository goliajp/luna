//! The program generator shared by the PUC differential targets:
//! side-effect-free expressions printed one per line.

use arbitrary::Arbitrary;
use luna_core::version::LuaVersion;
use std::fmt::Write;
use std::io::Write as IoWrite;
use std::process::{Command, Output, Stdio};

#[derive(Debug)]
pub(crate) enum Expr {
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
    // The variants below tostring-wrap inputs or guard cross-engine
    // semantic drift (negative-exponent power → complex; bad
    // format spec → runtime err; nil values via TableSet → key
    // deletion etc).
    StringConcat(Box<Expr>, Box<Expr>),
    StringFormat(Box<Expr>),
    TableGet(Box<Expr>),
    TableSet(Box<Expr>, Box<Expr>),
    Pow(Box<Expr>, Box<Expr>),
}

/// Deepest level `render` prints; nodes below it would never be seen.
const MAX_DEPTH: u32 = 4;

/// Upper bound on one rendered expression (about 7.2 KB: `Mod` at every
/// level). Reserving it up front keeps buffer growth out of the recursion,
/// where each call path would be one more allocation stack under ASan.
const MAX_EXPR_LEN: usize = 8 * 1024;

impl<'a> Arbitrary<'a> for Expr {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        Expr::arbitrary_at(u, 0)
    }
}

impl Expr {
    // Under ASan every distinct allocation stack stays in the stack depot for
    // good. The derived impl recursed as deep as the input allowed and boxed
    // children from a different call site per variant, so long runs kept
    // adding stacks until RSS hit the limit. Children are boxed at one site
    // here, and only as deep as render prints.
    fn arbitrary_at(u: &mut arbitrary::Unstructured<'_>, depth: u32) -> arbitrary::Result<Self> {
        if depth > MAX_DEPTH {
            return Ok(Expr::Int(0));
        }
        let variant = u.choose_index(16)?;
        let arity = match variant {
            0..=5 => 0,
            12 | 13 => 1,
            _ => 2,
        };
        let mut kids: [Option<Box<Expr>>; 2] = [None, None];
        for kid in kids.iter_mut().take(arity) {
            *kid = Some(Box::new(Expr::arbitrary_at(u, depth + 1)?));
        }
        let [l, r] = kids;
        let (l, r) = (|| l.unwrap(), || r.unwrap());
        Ok(match variant {
            0 => Expr::Int(u.arbitrary()?),
            1 => Expr::Float(u.arbitrary()?),
            2 => Expr::Nil,
            3 => Expr::True,
            4 => Expr::False,
            5 => Expr::Var(u.arbitrary()?),
            6 => Expr::Add(l(), r()),
            7 => Expr::Sub(l(), r()),
            8 => Expr::Mul(l(), r()),
            9 => Expr::Mod(l(), r()),
            10 => Expr::Lt(l(), r()),
            11 => Expr::StringConcat(l(), r()),
            12 => Expr::StringFormat(l()),
            13 => Expr::TableGet(l()),
            14 => Expr::TableSet(l(), r()),
            _ => Expr::Pow(l(), r()),
        })
    }
}

/// Floats restricted to a narrow non-pathological range — PUC's
/// `tostring` formatting of NaN / Inf / very-small / very-large
/// floats has corner-case spelling drift vs luna.
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
pub(crate) struct Program {
    prints: Vec<Expr>,
}

fn render_expr(buf: &mut String, e: &Expr, depth: u32, st: Style) {
    assert!(depth <= MAX_DEPTH + 1, "generated expression deeper than render prints");
    match e {
        Expr::Int(i) => write!(buf, "({i})").unwrap(),
        Expr::Float(NormalFloat(f)) => write!(buf, "({})", f).unwrap(),
        Expr::Nil => buf.push_str("nil"),
        Expr::True => buf.push_str("true"),
        Expr::False => buf.push_str("false"),
        Expr::Var(v) => buf.push_str(v.name()),
        Expr::Add(l, r) => match st.add.helper("add") {
            Some(f) => call(buf, &f, l, r, depth, st),
            None => bin(buf, "+", l, r, depth, st),
        },
        Expr::Sub(l, r) => bin(buf, "-", l, r, depth, st),
        Expr::Mul(l, r) => match st.mul.helper("mul") {
            Some(f) => call(buf, &f, l, r, depth, st),
            None => bin(buf, "*", l, r, depth, st),
        },
        Expr::Mod(l, r) => {
            // Guard divisor != 0 to avoid luna-vs-PUC error-message
            // wording drift.
            buf.push('(');
            render_expr(buf, l, depth + 1, st);
            buf.push_str(" % ((");
            render_expr(buf, r, depth + 1, st);
            buf.push_str(") ~= 0 and (");
            render_expr(buf, r, depth + 1, st);
            buf.push_str(") or 1))");
        }
        Expr::Lt(l, r) => bin(buf, "<", l, r, depth, st),
        Expr::StringConcat(l, r) => {
            buf.push_str("(tostring(");
            render_expr(buf, l, depth + 1, st);
            buf.push_str(") .. tostring(");
            render_expr(buf, r, depth + 1, st);
            buf.push_str("))");
        }
        Expr::StringFormat(e) => {
            // %d guarded with (tonumber(x) or 0) → math.floor →
            // integer string. PUC + luna agree on this contract;
            // any divergence is a real luna bug.
            buf.push_str("string.format('%d', math.floor(tonumber(");
            render_expr(buf, e, depth + 1, st);
            buf.push_str(") or 0))");
        }
        Expr::TableGet(e) => {
            // Key derived via tostring so any Value type works;
            // missing keys yield nil identically in both engines.
            buf.push_str("(t[tostring(");
            render_expr(buf, e, depth + 1, st);
            buf.push_str(")])");
        }
        Expr::TableSet(l, r) => {
            // wrap in IIFE so the assignment doesn't leak side
            // effects across prints. nil-value setter still
            // deletes the key — both engines agree, no need to
            // guard.
            buf.push_str("((function() local k = tostring(");
            render_expr(buf, l, depth + 1, st);
            buf.push_str(") local v = ");
            render_expr(buf, r, depth + 1, st);
            buf.push_str(" t[k] = v return v end)())");
        }
        Expr::Pow(l, r) => {
            // Exponent bounded to 0..3 via `((R) % 4)` so a
            // negative or fractional R doesn't yield complex /
            // NaN with cross-engine formatting drift.
            buf.push_str("((");
            render_expr(buf, l, depth + 1, st);
            buf.push_str(") ^ ((");
            render_expr(buf, r, depth + 1, st);
            buf.push_str(") % 4))");
        }
    }
}

fn bin(buf: &mut String, op: &str, l: &Expr, r: &Expr, depth: u32, st: Style) {
    buf.push('(');
    render_expr(buf, l, depth + 1, st);
    write!(buf, " {} ", op).unwrap();
    render_expr(buf, r, depth + 1, st);
    buf.push(')');
}

fn call(buf: &mut String, f: &str, l: &Expr, r: &Expr, depth: u32, st: Style) {
    write!(buf, "{f}(").unwrap();
    render_expr(buf, l, depth + 1, st);
    buf.push_str(", ");
    render_expr(buf, r, depth + 1, st);
    buf.push(')');
}

/// Which operand a `+` or `*` of two NaNs returns. PUC's choice is not
/// Lua's: x86 `addsd` / `mulsd` return the NaN in the destination
/// register, and gcc picks which operand goes there separately for each
/// PUC version (5.3 and 5.5 add and 5.3-5.5 multiply return the second).
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum NanPick {
    /// the operator as written
    Native,
    First,
    Second,
}

impl NanPick {
    fn helper(self, op: &str) -> Option<String> {
        match self {
            NanPick::Native => None,
            NanPick::First => Some(format!("__{op}_first")),
            NanPick::Second => Some(format!("__{op}_second")),
        }
    }
}

#[derive(Clone, Copy)]
struct Style {
    add: NanPick,
    mul: NanPick,
}

/// The helpers `render_with` calls: the operation itself, except that two
/// NaN operands give the one the name says.
const NAN_PICK_HELPERS: &str = "\
local function __add_first(x, y) local r = x + y if x ~= x and y ~= y then return x end return r end
local function __add_second(x, y) local r = x + y if x ~= x and y ~= y then return y end return r end
local function __mul_first(x, y) local r = x * y if x ~= x and y ~= y then return x end return r end
local function __mul_second(x, y) local r = x * y if x ~= x and y ~= y then return y end return r end
";

pub(crate) fn render(p: &Program) -> String {
    render_with(p, NanPick::Native, NanPick::Native)
}

/// `render`, with `+` and `*` written as calls that fix which NaN they
/// return when both operands are NaNs (`Native` keeps the operator).
pub(crate) fn render_with(p: &Program, add: NanPick, mul: NanPick) -> String {
    let st = Style { add, mul };
    let mut buf = String::new();
    if add != NanPick::Native || mul != NanPick::Native {
        buf.push_str(NAN_PICK_HELPERS);
    }
    // `t` injected for the TableGet / TableSet variants.
    buf.push_str("local a, b, c = 1, 2, 3\nlocal t = {}\n");
    for e in p.prints.iter().take(16) {
        buf.push_str("print(");
        buf.reserve(MAX_EXPR_LEN);
        let start = buf.len();
        render_expr(&mut buf, e, 0, st);
        // the helper calls are longer than the operators they replace
        assert!(
            st.add != NanPick::Native
                || st.mul != NanPick::Native
                || buf.len() - start <= MAX_EXPR_LEN,
            "MAX_EXPR_LEN too small"
        );
        buf.push_str(")\n");
    }
    buf
}

/// The dialect luna runs: `$LUNA_FUZZ_DIALECT` (`5.1` … `5.5`, default `5.5`).
pub(crate) fn dialect() -> LuaVersion {
    match std::env::var("LUNA_FUZZ_DIALECT").as_deref() {
        Ok("5.1") => LuaVersion::Lua51,
        Ok("5.2") => LuaVersion::Lua52,
        Ok("5.3") => LuaVersion::Lua53,
        Ok("5.4") => LuaVersion::Lua54,
        _ => LuaVersion::Lua55,
    }
}

/// Runs `$PUC_LUA -` with `input` on stdin; `None` when `$PUC_LUA` is
/// unset or does not start.
pub(crate) fn run_puc(input: &[u8]) -> Option<Output> {
    let bin = std::env::var("PUC_LUA").ok()?;
    let mut child = Command::new(&bin)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().ok()
}

pub(crate) fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end_matches('\n').to_string()
}
