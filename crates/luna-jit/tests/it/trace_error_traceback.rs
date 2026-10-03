//! Tracebacks of errors raised while compiled code runs: a trace leaves to
//! the interpreter at the operation that fails, rebuilding the frames of
//! the calls it inlined, and the traceback must read as if the interpreter
//! had run everything. The programs and PUC's text are luna-core's
//! `tests/error_traceback/` set (see `luna-core/tests/it/error_traceback.rs`);
//! here they run with the hot thresholds at 2, under each trace tier and
//! with the method JIT on.

use std::path::PathBuf;

use luna_jit::jit::trace::TraceTier;
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const DIALECTS: [(&str, LuaVersion); 5] = [
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

/// Programs whose loop runs in a trace when the error is raised, in every
/// dialect: the error leaves through a trace exit, with inlined frames
/// (`hot_inline_*`) to rebuild.
const RUN_IN_TRACES: [&str; 6] = [
    "hot_inline_arith",
    "hot_inline_deep",
    "hot_inline_method",
    "hot_inline_nil",
    "hot_loop",
    "hot_rec",
];

#[derive(Clone, Copy, Debug)]
enum Mode {
    Interp,
    Trace(TraceTier, u32),
    MethodAndTrace,
}

const MODES: [Mode; 5] = [
    Mode::Trace(TraceTier::Baseline, 0),
    Mode::Trace(TraceTier::Optimizing, 0),
    Mode::Trace(TraceTier::Auto, 1),
    Mode::Trace(TraceTier::Auto, 50),
    Mode::MethodAndTrace,
];

fn vm(v: LuaVersion, mode: Mode) -> Vm {
    let mut vm = luna_jit::new_with_jit(v);
    match mode {
        Mode::Interp => {
            vm.set_jit_enabled(false);
            vm.set_trace_jit_enabled(false);
        }
        Mode::Trace(tier, up_at) => {
            vm.set_jit_enabled(false);
            vm.set_trace_tier(tier);
            vm.set_trace_tier_up_at(up_at);
        }
        Mode::MethodAndTrace => {}
    }
    vm.jit.trace_hot_threshold = 2;
    vm.jit.call_hot_threshold = 2;
    vm
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../luna-core/tests/error_traceback")
}

fn source(name: &str) -> Vec<u8> {
    std::fs::read(fixture_dir().join(format!("{name}.lua"))).expect("read program")
}

fn expected(name: &str, dialect: &str) -> String {
    let text = std::fs::read_to_string(fixture_dir().join(format!("{name}.txt"))).expect("read");
    let head = format!("== {dialect}\n");
    let start = text.find(&head).expect("dialect section") + head.len();
    let end = text[start..]
        .find("\n== ")
        .map_or(text.len(), |i| start + i);
    text[start..end].trim_end_matches('\n').to_string()
}

fn programs() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixture_dir())
        .expect("fixture dir")
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_suffix(".lua").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

fn snapshot(vm: &mut Vm, name: &str) -> String {
    let main = vm
        .load(&source(name), format!("@{name}.lua").as_bytes())
        .unwrap_or_else(|e| panic!("{name}: {e:?}"));
    match vm.call_value(Value::Closure(main), &[]) {
        Ok(_) => panic!("{name}: ran without an error"),
        Err(_) => vm.take_error_traceback().expect("traceback taken"),
    }
}

#[test]
fn snapshots_match_puc_in_compiled_code() {
    let mut failures = Vec::new();
    for mode in MODES {
        for name in programs() {
            for (dialect, v) in DIALECTS {
                let mut vm = vm(v, mode);
                let got = snapshot(&mut vm, &name);
                let want = expected(&name, dialect);
                if got != want {
                    failures.push(format!(
                        "{mode:?} {name} {dialect}\n--- puc\n{want}\n--- luna\n{got}"
                    ));
                }
                let entered = vm.jit.counters.dispatched > 0;
                if RUN_IN_TRACES.contains(&name.as_str()) && !entered {
                    failures.push(format!("{mode:?} {name} {dialect}: no trace ran"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// `debug.traceback` as `xpcall`'s handler, run where the error leaves a
/// trace, lists the same levels as under the interpreter (whose text the
/// diff_puc fixtures pin against PUC).
#[test]
fn debug_traceback_in_a_handler_matches_the_interpreter() {
    const RUNNER: &str = "local f = ...\n\
        return select(2, xpcall(f, debug.traceback))";
    let run = |vm: &mut Vm, name: &str| -> String {
        let f = vm
            .load(&source(name), format!("@{name}.lua").as_bytes())
            .expect("load");
        let r = vm.load(RUNNER.as_bytes(), b"=runner").expect("load runner");
        let out = vm
            .call_value(Value::Closure(r), &[Value::Closure(f)])
            .expect("protected");
        match out.first() {
            Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            other => format!("{other:?}"),
        }
    };
    for name in RUN_IN_TRACES {
        for (dialect, v) in DIALECTS {
            let want = run(&mut vm(v, Mode::Interp), name);
            for mode in MODES {
                let mut jit = vm(v, mode);
                let got = run(&mut jit, name);
                assert_eq!(got, want, "{mode:?} {name} {dialect}");
                assert!(
                    jit.jit.counters.dispatched > 0,
                    "{mode:?} {name} {dialect}: no trace ran"
                );
            }
        }
    }
}
