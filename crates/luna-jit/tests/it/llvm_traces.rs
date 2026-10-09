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
    run_in(LuaVersion::Lua54, backend, tier, tier_up_at, src)
}

fn run_in(
    v: LuaVersion,
    backend: Backend,
    tier: TraceTier,
    tier_up_at: Option<u32>,
    src: &str,
) -> Run {
    let src = &repeated(src);
    let mut vm = luna_jit::new_with_jit(v);
    match backend {
        Backend::Interpreter => vm.install_null_jit(),
        Backend::Cranelift => vm.install_default_jit(),
        // tier-ups compiled before the trace runs on, so the counts do not
        // depend on how fast the compile thread is
        Backend::Llvm => luna_jit::install_llvm_backend_with(
            &mut vm,
            luna_jit::jit_backend::LlvmBackend { llvm_after: None },
        ),
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

/// Table reads and writes with a constant operand, a constant key and an
/// upvalue table, whose forms differ per dialect.
const TABLE_OPERANDS: &str = "
    local u = {0, 0}
    local function f(n)
      local t, s = {}, 0
      for i = 1, n do
        t.a = 1.5 t[1] = 2 t[i] = 0.25 t[2.5] = i t[-3] = 4
        u[i] = i u[1.5] = 2 u.x = i
        s = s + t.a + t[1] + t[i] + t[2.5] + t[-3] + u[i] + u[1.5] + u.x
      end
      return s
    end
    return tostring(f(5000))";

#[test]
fn llvm_compiles_table_constant_operands_in_every_dialect() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let opt = TraceTier::Optimizing;
        let interp = run_in(v, Backend::Interpreter, opt, None, TABLE_OPERANDS);
        let cl = run_in(v, Backend::Cranelift, opt, None, TABLE_OPERANDS);
        let ll = run_in(v, Backend::Llvm, opt, None, TABLE_OPERANDS);
        assert_eq!(ll.out, interp.out, "{v:?}: LLVM result");
        assert_eq!(cl.out, interp.out, "{v:?}: Cranelift result");
        assert!(ll.compiled > 0 && ll.failed == 0, "{v:?}: {ll:?}");
        assert_eq!((ll.compiled, ll.failed), (cl.compiled, cl.failed), "{v:?}");
        assert!(ll.llvm > 0 && ll.llvm == ll.codegen, "{v:?}: {ll:?}");
        assert!(ll.dispatched > 0, "{v:?}: {ll:?}");
    }
}

/// Each dialect's layout of both `for` loops, and constant operands on
/// either side of an operator (any constant before 5.4).
const LOOPS_AND_OPERANDS: &str = "
    local function f(n)
      local t, s = {}, 0
      for i = 1, n do t[i] = i % 7 end
      for i, v in ipairs(t) do s = s + v + (1 - i) % 5 end
      for k, v in pairs(t) do if 3 >= v then s = s + 1 end end
      for x = 0.5, 200.5 do if x < 1e300 and nil ~= x then s = s + 2 ^ (x % 3) end end
      return s
    end
    return tostring(f(3000))";

#[test]
fn llvm_compiles_each_dialects_loops_and_constant_operands() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let opt = TraceTier::Optimizing;
        let interp = run_in(v, Backend::Interpreter, opt, None, LOOPS_AND_OPERANDS);
        let cl = run_in(v, Backend::Cranelift, opt, None, LOOPS_AND_OPERANDS);
        let ll = run_in(v, Backend::Llvm, opt, None, LOOPS_AND_OPERANDS);
        assert_eq!(ll.out, interp.out, "{v:?}: LLVM result");
        assert_eq!(cl.out, interp.out, "{v:?}: Cranelift result");
        assert!(ll.compiled > 0, "{v:?}: {ll:?}");
        assert_eq!((ll.compiled, ll.failed), (cl.compiled, cl.failed), "{v:?}");
        assert!(ll.llvm > 0 && ll.llvm == ll.codegen, "{v:?}: {ll:?}");
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

/// By default a hot trace moves to Cranelift's code, then, once it has run
/// that for a while, is compiled by LLVM on the compile thread, and the Vm
/// installs the LLVM code once it is ready.
#[test]
fn background_tier_up_installs_llvm_code() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend(&mut vm);
    vm.set_trace_tier(TraceTier::Auto);
    vm.set_trace_tier_up_at(64);
    vm.eval(
        "function RUN()
           local s = 0
           for i = 1, 20000 do s = s + (i * 3) % 7 end
           return s
         end",
    )
    .expect("defines RUN");
    let before = luna_jit::jit_backend::trace::llvm_codegen_count();
    let started = std::time::Instant::now();
    while luna_jit::jit_backend::trace::llvm_codegen_count() == before {
        let r = vm.eval("return RUN()").expect("runs");
        assert_eq!(show(&r), "Some(Int(60000))");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(60),
            "the LLVM code was never installed"
        );
    }
    let r = vm.eval("return RUN()").expect("runs on LLVM code");
    assert_eq!(show(&r), "Some(Int(60000))");
}

/// A loop entered once per call, waiting for LLVM's code, takes it at the
/// first entry after it is ready, not after many more calls.
#[test]
fn background_tier_up_reaches_a_loop_entered_once_per_call() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    luna_jit::install_llvm_backend(&mut vm);
    vm.set_trace_tier(TraceTier::Auto);
    vm.set_trace_tier_up_at(64);
    vm.eval(
        "function RUN()
           local s = 0
           for i = 1, 20000 do s = s + (i * 3) % 7 end
           return s
         end",
    )
    .expect("defines RUN");
    let before = luna_jit::jit_backend::trace::llvm_codegen_count();
    let mut calls = 0;
    while luna_jit::jit_backend::trace::llvm_codegen_count() == before {
        let r = vm.eval("return RUN()").expect("runs");
        assert_eq!(show(&r), "Some(Int(60000))");
        calls += 1;
        // the wait (20 ms) and LLVM's compile, with room for a slow machine
        assert!(calls < 50, "LLVM's code not taken after {calls} calls");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
