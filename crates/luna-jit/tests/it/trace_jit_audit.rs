//! Trace JIT correctness audit — exercises hot-loop / side-trace /
//! invalidation / cross-trace shapes that the method-JIT audit doesn't
//! engage. Each test:
//! 1. Runs a Lua program that SHOULD engage trace JIT recording.
//! 2. Verifies the result is correct (Lua-level + Value variant).
//! 3. Optionally inspects trace counters to confirm engagement (so a
//!    silently-bailed trace doesn't pass as "Lua-level correct via
//!    interp fallback").
//!
//! These tests are dialect-light — most trace JIT shapes are dialect-
//! agnostic, but a few are 5.4+ only (ForLoop pre-5.3 uses a different
//! BC layout that trace JIT bails on).

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

fn vm_default(version: LuaVersion) -> Vm {
    luna_jit::new_with_jit(version)
}

/// luna's default Vm enables BOTH method JIT and trace JIT. Method
/// JIT consumes a proto on first call if it can; trace JIT only sees
/// protos method JIT BAILED on. To audit trace JIT specifically, we
/// must disable method JIT so trace JIT runs.
fn vm_trace_only(version: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm
}

fn eval_one(vm: &mut Vm, src: &str) -> Value {
    let cl = vm.load(src.as_bytes(), b"=trace-audit").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("call");
    r.into_iter().next().unwrap_or(Value::Nil)
}

const DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua51, "5.1"),
    (LuaVersion::Lua52, "5.2"),
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

/// 5.4 / 5.5 only — pre53 ForLoop layout differs and trace JIT bails
/// (`src/jit/trace.rs:4410`).
const POST53_DIALECTS: &[(LuaVersion, &str)] = &[
    (LuaVersion::Lua53, "5.3"),
    (LuaVersion::Lua54, "5.4"),
    (LuaVersion::Lua55, "5.5"),
];

mod for_loop_shapes;
mod hot_loops;
mod pre53_fallback;
mod run_parity;
