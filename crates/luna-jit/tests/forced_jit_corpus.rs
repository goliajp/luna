//! The diff_puc corpus with the JIT engaged early, against the
//! interpreter.
//!
//! luna-core's `diff_puc` pins every fixture to PUC on the interpreter,
//! and the JIT-equipped CLI runs them too, but almost none loops 64
//! times, so the trace JIT never records anything there. This runs each
//! fixture of `luna-core/tests/diff_puc/5.x` on a Vm whose trace hot
//! thresholds are lowered (a loop is recorded after a few back-edges, a
//! function after a few calls) and requires the output, or the error,
//! the interpreter gives. The method JIT compiles on the first call
//! anyway.
//!
//! The run must be able to fail, so each dialect checks that the method
//! JIT compiled something and, where traces are compiled at all (5.2+;
//! 5.1 numeric `for` loops are not), that a trace was dispatched.

use std::path::{Path, PathBuf};

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const DIALECTS: &[(&str, LuaVersion)] = &[
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

/// Back-edges / calls before a trace is recorded. Several values, since
/// what gets recorded (and which iteration a trace starts on) depends on
/// it.
const HOT: &[u32] = &[1, 2, 5];

fn fixtures(dialect: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../luna-core/tests/diff_puc")
        .join(dialect);
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    out.sort();
    out
}

/// Run `source` with `print` / `io.write` captured: the output, or the
/// error as the standalone interpreter would show it.
fn run(vm: &mut Vm, source: &[u8]) -> Result<String, String> {
    vm.eval(
        r#"
_G.__out = ""
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out = _G.__out .. table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out = _G.__out .. table.concat(t)
end
"#,
    )
    .expect("capture preamble");
    let f = vm
        .load(source, b"=fixture")
        .map_err(|e| String::from_utf8_lossy(&e.msg).into_owned())?;
    if let Err(e) = vm.call_value(Value::Closure(f), &[]) {
        return Err(vm.error_display(&e));
    }
    match vm.eval("return _G.__out").expect("buffer").first() {
        Some(Value::Str(s)) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        other => panic!("expected the capture buffer, got {other:?}"),
    }
}

/// Addresses (`table: 0x…`) differ between two Vms.
fn mask_addresses(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("0x") {
        out.push_str(&rest[..i]);
        let hex = rest[i + 2..]
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len() - i - 2);
        out.push_str("ADDR");
        rest = &rest[i + 2 + hex..];
    }
    out.push_str(rest);
    out
}

fn masked(r: Result<String, String>) -> Result<String, String> {
    r.map(|s| mask_addresses(&s)).map_err(|s| mask_addresses(&s))
}

#[test]
fn corpus_runs_the_same_with_the_jit_engaged_early() {
    let mut failed = Vec::new();
    for &(dialect, version) in DIALECTS {
        let mut dispatched = 0u64;
        let mut compiled_chunks = 0usize;
        for f in &fixtures(dialect) {
            let source = std::fs::read(f).expect("read fixture");
            let mut interp_vm = luna_jit::new_with_jit(version);
            interp_vm.set_jit_enabled(false);
            interp_vm.set_trace_jit_enabled(false);
            let interp = masked(run(&mut interp_vm, &source));
            for &hot in HOT {
                let mut vm = luna_jit::new_with_jit(version);
                vm.jit.trace_hot_threshold = hot;
                vm.jit.call_hot_threshold = hot;
                let jit = masked(run(&mut vm, &source));
                dispatched += vm.trace_dispatched_count();
                compiled_chunks += luna_jit::jit::cache_entry_count(&vm);
                if jit != interp {
                    failed.push(format!(
                        "{} (hot = {hot}):\n  interpreter: {interp:?}\n  jit:         {jit:?}",
                        f.display()
                    ));
                }
            }
        }
        eprintln!(
            "[forced_jit_corpus] {dialect}: {compiled_chunks} method chunks compiled, \
             {dispatched} trace dispatches"
        );
        assert!(
            compiled_chunks > 0,
            "{dialect}: the method JIT compiled nothing, so nothing was compared"
        );
        assert!(
            version == LuaVersion::Lua51 || dispatched > 0,
            "{dialect}: no trace was dispatched, so the trace JIT was not compared"
        );
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
