//! Float comparisons against NaN in compiled code. Every ordered
//! comparison with a NaN operand is false, so `not (a < b)` is true where
//! `a >= b` is false: a compiled branch that negates a comparison by
//! flipping its operator goes the wrong way. The function below compiles
//! with finite operands and then meets NaN.

use luna_jit::LuaVersion;
use luna_jit::runtime::Value;

#[derive(Clone, Copy)]
enum Jit {
    Off,
    Method,
    Trace,
}

fn run(version: LuaVersion, src: &str, jit: Jit) -> (String, u64) {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(matches!(jit, Jit::Method));
    vm.set_trace_jit_enabled(matches!(jit, Jit::Trace));
    let out = match vm.eval(src) {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => panic!("snippet must return a string, got {other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, vm.trace_dispatched_count())
}

const SHAPES: [&str; 7] = [
    "x OP y",
    "y OP x",
    "x OP 0.5",
    "0.5 OP x",
    "x OP 2.5",
    "not (x OP y)",
    "not (x OP 0.5)",
];

/// `x` is 1.0 except every 50th call, where it is NaN; `y` is below or
/// above it, so each shape is compiled once taken and once not taken.
fn check(op: &str, traced: bool) {
    let mut dispatched_any = [false; 2];
    for shape in SHAPES {
        let cond = shape.replace("OP", op);
        for y in ["0.5", "2.5"] {
            let src = format!(
                "local function f(x, y)
                   local c = 0
                   for i = 1, 3 do if {cond} then c = c + 1 end end
                   return c
                 end
                 local c, y = 0, {y}
                 for i = 1, 400 do
                   local x = (i % 50) / (i % 50)
                   c = c + f(x, y)
                 end
                 return tostring(c)"
            );
            for (n, v) in [LuaVersion::Lua54, LuaVersion::Lua55]
                .into_iter()
                .enumerate()
            {
                let (interp, _) = run(v, &src, Jit::Off);
                let (method, _) = run(v, &src, Jit::Method);
                let (trace, dispatched) = run(v, &src, Jit::Trace);
                let at = format!("{v:?} `{cond}` y = {y}");
                assert_eq!(
                    method, interp,
                    "{at}: method JIT differs from the interpreter"
                );
                assert_eq!(
                    trace, interp,
                    "{at}: trace JIT differs from the interpreter"
                );
                dispatched_any[n] |= dispatched > 0;
            }
        }
    }
    // no shape of `~=` runs as a trace today; it is checked all the same
    if traced {
        assert_eq!(dispatched_any, [true; 2], "`{op}`: no trace was dispatched");
    }
}

#[test]
fn less_than() {
    check("<", true);
}

#[test]
fn less_equal() {
    check("<=", true);
}

#[test]
fn greater_than() {
    check(">", true);
}

#[test]
fn greater_equal() {
    check(">=", true);
}

#[test]
fn equal() {
    check("==", true);
}

#[test]
fn not_equal() {
    check("~=", false);
}
