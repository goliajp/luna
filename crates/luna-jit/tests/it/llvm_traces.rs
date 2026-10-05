//! The LLVM backend compiles traces with the shared trace lowering, and
//! those traces run.
//!
//! Each script runs on the interpreter, on the Cranelift backend and on the
//! LLVM backend, with every trace compiled by the optimizing tier (and, for
//! [`tier_up_reaches_llvm`], by the baseline tier first). The LLVM run must
//! return what the interpreter returns, compile as many traces as Cranelift,
//! with LLVM producing the code, and dispatch them.

use luna_jit::VmExt;
use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;

/// A hot loop calling a function: the shape that compiled no trace on the
/// LLVM backend before it used the shared lowering.
const CALL_LOOP: &str = "
    local function f(x, y)
      local z = x * 3 + y
      if z % 2 == 0 then z = z // 2 else z = z - 1 end
      return z & 1023
    end
    local s = 0
    for i = 1, 20000 do s = s + f(i, s & 255) end
    return s";

const SCRIPTS: &[(&str, &str)] = &[
    ("call_loop", CALL_LOOP),
    (
        "tables_and_fields",
        "local t = {}
         for i = 1, 5000 do t[i] = {x = i, y = i * 0.5} end
         local sx, sy = 0, 0.0
         for i = 1, #t do local p = t[i]; sx = sx + p.x; sy = sy + p.y end
         return sx .. ' ' .. sy",
    ),
    (
        "branches_and_side_exits",
        "local a, b = 0, 0
         for i = 1, 30000 do
           if i % 3 == 0 then a = a + i elseif i % 5 == 0 then b = b - i else a = a ~ i end
         end
         return a .. ' ' .. b",
    ),
    (
        // the trace computes on raw payloads: nil, false and the integer
        // 0 must still compare as Lua says
        "zero_nil_false",
        "local t = {0, nil, 0, false}
         local n = 0
         for i = 1, 20000 do if t[(i % 4) + 1] == 0 then n = n + 1 end end
         return n",
    ),
    (
        // LLVM must not fold `0 / 0` to a NaN of another sign than the
        // machine's division makes (Lua prints the sign)
        "nan_from_known_operands",
        "local t = {}
         for i = 1, 5000 do t[i] = (i - i) / (i - i) end
         local a = tostring(t[5000])
         for i = 1, 5000 do t[i] = -((i - i) / (i - i)) end
         return a .. ' ' .. tostring(t[5000])",
    ),
    (
        "strings_and_collection",
        "collectgarbage('setpause', 0)
         local parts = {}
         for i = 1, 3000 do parts[#parts + 1] = 'k' .. i .. ':' .. (i * 3) end
         local n = 0
         for _, s in ipairs(parts) do n = n + #s end
         return n .. ' ' .. parts[1234]",
    ),
];

fn show(r: &[Value]) -> String {
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => format!("{other:?}"),
    }
}

#[derive(Debug)]
struct Run {
    out: String,
    compiled: u64,
    failed: u64,
    dispatched: u64,
    /// Traces given machine code, and how many of them by LLVM.
    codegen: u64,
    llvm: u64,
}

enum Backend {
    Interpreter,
    Cranelift,
    Llvm,
}

/// `body` as a function run a few times: a loop is entered as a trace the
/// next time it starts after its trace was compiled.
fn repeated(body: &str) -> String {
    format!("local function run() {body} end\nlocal r\nfor _ = 1, 4 do r = run() end\nreturn r")
}

fn run(backend: Backend, tier: TraceTier, tier_up_at: Option<u32>, src: &str) -> Run {
    let src = &repeated(src);
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    match backend {
        Backend::Interpreter => vm.install_null_jit(),
        Backend::Cranelift => vm.install_default_jit(),
        Backend::Llvm => luna_jit::install_llvm_backend(&mut vm),
    }
    vm.set_trace_tier(tier);
    if let Some(n) = tier_up_at {
        vm.set_trace_tier_up_at(n);
    }
    let codegen_before = luna_jit::jit_backend::trace::trace_codegen_count();
    let llvm_before = luna_jit::jit_backend::trace::llvm_codegen_count();
    let out = match vm.eval(src) {
        Ok(r) => show(&r),
        Err(e) => format!("error: {e}"),
    };
    Run {
        out,
        compiled: vm.trace_compiled_count(),
        failed: vm.trace_compile_failed_count(),
        dispatched: vm.trace_dispatched_count(),
        codegen: luna_jit::jit_backend::trace::trace_codegen_count() - codegen_before,
        llvm: luna_jit::jit_backend::trace::llvm_codegen_count() - llvm_before,
    }
}

#[test]
fn llvm_compiles_and_dispatches_the_traces_cranelift_does() {
    for &(name, src) in SCRIPTS {
        let interp = run(Backend::Interpreter, TraceTier::Optimizing, None, src);
        let cl = run(Backend::Cranelift, TraceTier::Optimizing, None, src);
        let ll = run(Backend::Llvm, TraceTier::Optimizing, None, src);
        assert_eq!(ll.out, interp.out, "{name}: LLVM result");
        assert_eq!(cl.out, interp.out, "{name}: Cranelift result");
        assert!(ll.compiled > 0, "{name}: LLVM compiled no trace");
        assert_eq!(
            (ll.compiled, ll.failed),
            (cl.compiled, cl.failed),
            "{name}: LLVM and Cranelift compile different sets of traces"
        );
        assert!(
            ll.codegen > 0,
            "{name}: no trace got machine code: {ll:?} {cl:?}"
        );
        assert_eq!(ll.codegen, cl.codegen, "{name}: traces given code");
        assert_eq!(ll.llvm, ll.codegen, "{name}: traces LLVM compiled");
        assert!(ll.dispatched > 0, "{name}: no LLVM trace was dispatched");
    }
}

#[test]
fn hot_loop_with_a_call_runs_as_an_llvm_trace() {
    let ll = run(Backend::Llvm, TraceTier::Optimizing, None, CALL_LOOP);
    let interp = run(Backend::Interpreter, TraceTier::Optimizing, None, CALL_LOOP);
    assert_eq!(ll.out, interp.out);
    assert_eq!(ll.failed, 0, "a trace failed to compile");
    assert!(ll.llvm > 0 && ll.dispatched > 0, "{ll:?}");
}

/// With the default tiering the baseline tier compiles a trace first and
/// LLVM compiles it again once it is hot.
#[test]
fn tier_up_reaches_llvm() {
    for &(name, src) in SCRIPTS {
        let interp = run(Backend::Interpreter, TraceTier::Auto, None, src);
        let ll = run(Backend::Llvm, TraceTier::Auto, Some(64), src);
        assert_eq!(ll.out, interp.out, "{name}");
        assert!(ll.dispatched > 0, "{name}: no trace was dispatched: {ll:?}");
        assert!(ll.llvm > 0, "{name}: no trace reached LLVM");
    }
}
