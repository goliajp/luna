//! The instruction budget is a sandbox boundary with the JIT on as well:
//! while a budget is armed no compiled code runs, and once it is exhausted
//! every instruction raises the error again until the host arms a new one.
//! Each case runs under the interpreter, the method JIT, the trace JIT and
//! (with `llvm-jit`) the LLVM backend, with hot thresholds low enough that
//! the warm-up compiles.

use std::sync::mpsc;
use std::time::Duration;

use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::error::LuaErrorKind;

const BUDGET: i64 = 100_000;
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
enum Mode {
    Interpreter,
    MethodJit,
    TraceJit,
    #[cfg(feature = "llvm-jit")]
    Llvm,
}

const MODES: &[Mode] = &[
    Mode::Interpreter,
    Mode::MethodJit,
    Mode::TraceJit,
    #[cfg(feature = "llvm-jit")]
    Mode::Llvm,
];

#[derive(Debug, PartialEq)]
enum Outcome {
    Ok(String),
    Err { text: String, kind: LuaErrorKind },
    Timeout,
}

fn vm_for(mode: Mode, version: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_minimal_with_jit(version);
    vm.open_base();
    vm.open_table();
    match mode {
        Mode::Interpreter => vm.install_null_jit(),
        Mode::MethodJit => vm.set_trace_jit_enabled(false),
        Mode::TraceJit => vm.set_jit_enabled(false),
        #[cfg(feature = "llvm-jit")]
        Mode::Llvm => luna_jit::install_llvm_backend(&mut vm),
    }
    vm.jit.trace_hot_threshold = 2;
    vm.jit.call_hot_threshold = 2;
    vm
}

fn call(vm: &mut Vm, src: &str) -> Outcome {
    let cl = vm.load(src.as_bytes(), b"=case").expect("compile");
    match vm.call_value(Value::Closure(cl), &[]) {
        Ok(vals) => Outcome::Ok(match vals.first() {
            Some(Value::Int(i)) => i.to_string(),
            Some(Value::Float(f)) => format!("{f}"),
            other => format!("{other:?}"),
        }),
        Err(e) => Outcome::Err {
            text: vm.error_text(&e),
            kind: vm.error_kind(),
        },
    }
}

fn bounded<F>(mode: Mode, version: LuaVersion, body: F) -> Outcome
where
    F: FnOnce(&mut Vm) -> Outcome + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut vm = vm_for(mode, version);
        let out = body(&mut vm);
        drop(vm);
        let _ = tx.send(out);
    });
    rx.recv_timeout(TIMEOUT).unwrap_or(Outcome::Timeout)
}

fn exhausted(mode: Mode, out: &Outcome) {
    match out {
        Outcome::Err { text, kind } => {
            assert!(
                text.contains("instruction budget exceeded"),
                "{mode:?}: {text}"
            );
            assert_eq!(*kind, LuaErrorKind::InstrBudget, "{mode:?}: {text}");
        }
        other => panic!("{mode:?}: expected the budget error, got {other:?}"),
    }
}

/// A counted loop in a function the method JIT compiles and the trace JIT
/// records; `sum(n)` is also the warm-up.
const SUM: &str = "
    function sum(n) local s = 0 for i = 1, n do s = s + i end return s end
    local acc = 0
    for k = 1, 50 do acc = acc + sum(100) end
    return acc";

#[test]
fn pcall_cannot_resume_with_the_jit_on() {
    for &mode in MODES {
        for version in [LuaVersion::Lua51, LuaVersion::Lua54] {
            let out = bounded(mode, version, |vm| {
                vm.set_instr_budget(Some(BUDGET));
                call(
                    vm,
                    "pcall(function() for i = 1, 1e9 do end end)
                     local s = 0
                     for i = 1, 1e9 do s = s + i end
                     return s",
                )
            });
            exhausted(mode, &out);
        }
    }
}

#[test]
fn warm_compiled_code_is_not_entered_under_a_budget() {
    for &mode in MODES {
        let out = bounded(mode, LuaVersion::Lua54, move |vm| {
            assert_eq!(call(vm, SUM), Outcome::Ok("252500".to_string()), "{mode:?}");
            if matches!(mode, Mode::TraceJit) {
                assert!(vm.trace_compiled_count() > 0, "{mode:?}: no trace compiled");
            }
            vm.set_instr_budget(Some(BUDGET));
            let out = call(vm, "return sum(1e9)");
            exhausted(mode, &out);
            // still exhausted: the compiled function is not an escape either
            let out = call(vm, "pcall(sum, 10) return sum(10)");
            exhausted(mode, &out);
            vm.set_instr_budget(Some(BUDGET));
            call(vm, "return sum(1000)")
        });
        assert_eq!(out, Outcome::Ok("500500".to_string()), "{mode:?}");
    }
}

#[test]
fn library_callback_under_the_jit_is_stopped() {
    for &mode in MODES {
        let out = bounded(mode, LuaVersion::Lua54, |vm| {
            vm.set_instr_budget(Some(BUDGET));
            call(
                vm,
                "local t = {3, 1, 2}
                 pcall(table.sort, t, function(a, b) for i = 1, 1e9 do end return a < b end)
                 local s = 0
                 for i = 1, 1e9 do s = s + i end
                 return s",
            )
        });
        exhausted(mode, &out);
    }
}
