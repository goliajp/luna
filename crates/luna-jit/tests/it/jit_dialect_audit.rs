// 3.14 used as float fixture in JIT dialect tests, not as π.
#![allow(clippy::approx_constant)]

//! Method JIT × dialect × Value-introspection audit.
//!
//! Each test runs ONE Op pattern (or small Op combination) under every
//! Lua dialect with method JIT enabled (default), then asserts the
//! returned Value's variant matches the dialect's type semantics.
//!
//! Method JIT bugs of the form "value is Lua-level correct but the
//! runtime Value variant is wrong" (tag drift) are invisible to
//! PUC-diff testing because Lua-level `type()` / arithmetic auto-
//! promote between Int and Float. This file catches them by
//! discriminating on the actual `Value` enum variant.
//!
//! Two latent bugs were caught and fixed this session by tests of
//! this shape:
//! 1. `Op::GetTable` defaulting result kind to Int regardless of
//!    dialect (5.1/5.2 returned `Int(0x4034...)` = f64 raw bits as
//!    Int instead of `Float(20.0)`). Fixed at src/jit/mod.rs:3016
//!    (dialect-aware default).
//! 2. 5.1's `luaK_nil` optimization skips `LoadNil` for an
//!    uninitialized local at function start; method JIT then read
//!    cranelift Variable's default `0` and silently arith'd it
//!    (`nil + 1 → 1` instead of raising). Fixed at
//!    src/jit/mod.rs::try_compile_int_chunk scan-init (pre-mark
//!    `is_nil_writer[num_params..max_stack] = true`).
//!
//! Naming: `audit_<op_or_pattern>` per test.

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

// ---------------------------------------------------------------------------
// Helpers — by default Vm has method JIT enabled, exercising the
// JIT bug surface. To re-test interp-only, set JIT off via the
// `interp_*` variants.

fn vm_default(version: LuaVersion) -> Vm {
    luna_jit::new_with_jit(version)
}

fn eval_one(vm: &mut Vm, src: &str) -> Value {
    let cl = vm.load(src.as_bytes(), b"=audit").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    r.into_iter().next().unwrap_or(Value::Nil)
}

fn is_pre_53(v: LuaVersion) -> bool {
    matches!(v, LuaVersion::Lua51 | LuaVersion::Lua52)
}

/// Assert numeric `n`. Under 5.3+ the variant must be `Int(n)` (strict,
/// since 5.3+ has the integer subtype). Under 5.1/5.2 accept either
/// `Int(n)` (luna's internal optimization for stdlib paths like `#tbl`,
/// `math.floor`) OR `Float(n as f64)` (PUC-strict) — both are
/// Lua-level "number" with value `n`, so `type()` and arithmetic
/// agree. Refusing the Int form here would false-positive on benign
/// tag drift; refusing the Float form would miss real bugs like the
/// `Int(raw_f64_bits_of_n)` shape the JIT GetTable bug produced.
///
/// To catch the raw-bits bug class explicitly, we also reject any Int
/// whose value differs from `n` and whose f64 bit-interpretation also
/// differs from `n` (i.e. neither an honest Int nor a tag-drift Float
/// re-interpreted as Int).
fn assert_strict_num(version: LuaVersion, actual: Value, n: i64, label: &str) {
    let ok = match actual {
        Value::Int(i) if i == n => true,
        Value::Float(f) if is_pre_53(version) && f == n as f64 => true,
        _ => false,
    };
    assert!(
        ok,
        "audit[{}]: expected number == {}, got {:?}",
        label, n, actual
    );
}

fn assert_strict_float(actual: Value, expected: f64, eps: f64, label: &str) {
    match actual {
        Value::Float(f) => assert!(
            (f - expected).abs() < eps,
            "audit[{}]: expected ≈{}, got Float({})",
            label,
            expected,
            f
        ),
        _ => panic!("audit[{}]: expected Float, got {:?}", label, actual),
    }
}

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "5.1"),
    (LuaVersion::Lua52, "5.2"),
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

mod core_ops;
mod set_paths;
mod wider_ops;
