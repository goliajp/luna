//! v2.0 Phase 1 CB-edge — compiler stress edges.
//!
//! Audit (`.dev/rfcs/v2.0-plan-state.md` §Phase 0 Track CB summary):
//! "compiler edge: long fn / deep nesting / many upvals / spread
//! call site — write 2-3 spot tests".
//!
//! These pin **compile + run** invariants at sizes the v1.x test
//! suite never hit. The luna compiler / dispatcher has known fields
//! sized for typical (≪100 LOC, ≪50-depth, ≪64-upval) shapes; the
//! tests below exercise the larger end so any silent overflow into
//! limit-field truncation surfaces here rather than from a dogfood
//! report.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// 2000-line function body (one statement per line, simple arithmetic).
/// Pins the compiler's per-function instruction buffer + line table
/// against MAX_OP / line-info shrink. Without this, a 5k-line user
/// script could silently lose its tail past whatever soft cap luna
/// enforces internally.
#[test]
fn compiler_long_fn_body_2000_statements() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut src = String::from("local n = 0\n");
    for _ in 0..2000 {
        src.push_str("n = n + 1\n");
    }
    src.push_str("return n\n");
    let r = vm.eval(&src).expect("2000-statement fn must compile + run");
    let n = match r.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int(2000), got {other:?}"),
    };
    assert_eq!(n, 2000, "2000 statements must each increment n");
}

/// 150-deep do-end block nesting. Pins the compiler's block scope
/// stack at the deep-but-not-PUC-capped end. PUC enforces
/// `LUAI_MAXCCALLS = 200` on syntactic nesting; luna matches. We
/// pin success at 150 and document the cap location via the
/// `cap` sibling test below — anything below 200 must compile +
/// run; anything ≥200 may cleanly error with a nesting-limit
/// diagnostic.
#[test]
fn compiler_deep_block_nesting_150() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut src = String::from("local result = 0\n");
    for _ in 0..150 {
        src.push_str("do\n");
    }
    src.push_str("result = 42\n");
    for _ in 0..150 {
        src.push_str("end\n");
    }
    src.push_str("return result\n");
    let r = vm
        .eval(&src)
        .expect("150-deep do-block nesting must compile + run");
    let result = match r.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int(42), got {other:?}"),
    };
    assert_eq!(result, 42);
}

/// 250-deep do-end block nesting — must reject with a clean parser
/// error, not panic / SIGSEGV. Pins the PUC `LUAI_MAXCCALLS = 200`
/// behavior so any future regression that silently lifts the cap
/// (or replaces the diagnostic with a panic) surfaces here.
#[test]
fn compiler_deep_block_nesting_cap_250_rejects_cleanly() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut src = String::from("local result = 0\n");
    for _ in 0..250 {
        src.push_str("do\n");
    }
    src.push_str("result = 42\n");
    for _ in 0..250 {
        src.push_str("end\n");
    }
    src.push_str("return result\n");
    let r = vm.eval(&src);
    assert!(
        r.is_err(),
        "250-deep nesting must error cleanly (PUC LUAI_MAXCCALLS=200 cap)"
    );
}

/// 60-deep arithmetic expression `(((...1+1...)+1)+1)`. Pins the
/// parser's expression-recursion depth; a soft stack-overflow guard
/// here should surface as a clean compile error, not a Rust panic /
/// SIGSEGV.
#[test]
fn compiler_deep_expression_nesting_60() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut src = String::from("return ");
    for _ in 0..60 {
        src.push('(');
    }
    src.push('1');
    for _ in 0..60 {
        src.push_str("+1)");
    }
    src.push('\n');
    let r = vm
        .eval(&src)
        .expect("60-deep paren expression must compile + run");
    let n = match r.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int(61), got {other:?}"),
    };
    assert_eq!(n, 61, "60 (+1) operations on initial 1 = 61");
}

/// A closure with 100 outer locals all captured as upvalues. Pins the
/// compiler's per-function upvalue table; PUC caps at 255, luna should
/// match. Below 255 must compile + the captured values must round-trip.
#[test]
fn compiler_many_upvalues_100() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    // Declare 100 locals at the outer scope, then capture them all
    // inside a closure that sums them — proving each upvalue was wired.
    let mut src = String::new();
    for i in 1..=100 {
        src.push_str(&format!("local v{i} = {i}\n"));
    }
    src.push_str("local function sum_all()\n  return ");
    for i in 1..=100 {
        if i > 1 {
            src.push_str(" + ");
        }
        src.push_str(&format!("v{i}"));
    }
    src.push_str("\nend\nreturn sum_all()\n");
    let r = vm
        .eval(&src)
        .expect("closure with 100 upvalues must compile + run");
    let sum = match r.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int(5050), got {other:?}"),
    };
    assert_eq!(
        sum, 5050,
        "sum of 1..100 must round-trip through 100 upvalues"
    );
}

/// Spread call site: `f(...)` forwarding a 50-arg vararg through one
/// call layer to a callee that returns its arg count. Pins the
/// dispatcher's vararg expansion at a non-trivial fan-out.
#[test]
fn compiler_spread_vararg_50_args() {
    let mut vm = Vm::new(LuaVersion::Lua55);
    let mut src = String::from(
        "
        local function count(...)
            return select('#', ...)
        end
        local function forward(...)
            return count(...)
        end
        return forward(",
    );
    for i in 1..=50 {
        if i > 1 {
            src.push_str(", ");
        }
        src.push_str(&i.to_string());
    }
    src.push_str(")\n");
    let r = vm
        .eval(&src)
        .expect("50-arg vararg forward must compile + run");
    let n = match r.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int(50), got {other:?}"),
    };
    assert_eq!(n, 50, "50 args must reach select('#', ...) intact");
}
