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

use luna_core::__private::VmJitState;
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

/// luna-core's fixtures too slow for a collection at every checkpoint in
/// one job, `(dialect, file name, shards)`: under `gc-stress` the corpus
/// run leaves them out and [`slow_fixture_shard`] runs them.
fn slow() -> Vec<(String, String, u64)> {
    let f = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../luna-core/tests/diff_puc/gc_stress_slow.txt");
    let text = std::fs::read_to_string(&f).unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            (
                w[0].to_string(),
                w[1].to_string(),
                w[2].parse().expect("a shard count"),
            )
        })
        .collect()
}

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

/// Addresses (`table: 0x…`, on Windows 16 upper-case hex digits) differ
/// between two Vms.
fn mask_addresses(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let hex_run = |from: usize| {
            b[from..]
                .iter()
                .take_while(|c| c.is_ascii_hexdigit())
                .count()
        };
        let n = if b[i..].starts_with(b"0x") {
            2 + hex_run(i + 2)
        } else {
            0
        };
        let windows = (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b[i..]
                .iter()
                .take_while(|c| {
                    c.is_ascii_digit() || c.is_ascii_uppercase() && c.is_ascii_hexdigit()
                })
                .count()
                == 16
            && b.get(i + 16).is_none_or(|c| !c.is_ascii_alphanumeric());
        if n > 2 || windows {
            out.push_str("ADDR");
            i += if windows { 16 } else { n };
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn masked(r: Result<String, String>) -> Result<String, String> {
    r.map(|s| mask_addresses(&s))
        .map_err(|s| mask_addresses(&s))
}

#[test]
fn corpus_runs_the_same_with_the_jit_engaged_early() {
    let mut failed = Vec::new();
    let slow = slow();
    assert!(!slow.is_empty(), "no slow fixtures listed");
    for &(dialect, version) in DIALECTS {
        let mut dispatched = 0u64;
        let mut compiled_chunks = 0usize;
        for f in &fixtures(dialect) {
            if cfg!(feature = "gc-stress")
                && slow.iter().any(|(d, n, _)| d == dialect && f.ends_with(n))
            {
                continue;
            }
            let source = std::fs::read(f).expect("read fixture");
            let _turn = super::trace_corpus::huge_turn(f);
            // one string hash seed for the runs compared (see `seeded`)
            let mut interp_vm = super::trace_corpus::seeded(version, 1);
            interp_vm.set_jit_enabled(false);
            interp_vm.set_trace_jit_enabled(false);
            let interp = masked(run(&mut interp_vm, &source));
            // one Vm at a time holds what a fixture loaded
            drop(interp_vm);
            for &hot in HOT {
                let mut vm = super::trace_corpus::seeded(version, 1);
                vm.jit_mut().trace_hot_threshold = hot;
                vm.jit_mut().call_hot_threshold = hot;
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

/// One shard of one slow fixture at one hot threshold, collecting at every
/// `LUNA_GC_STRESS_EVERY`th checkpoint from the `LUNA_GC_STRESS_OFFSET`th
/// (the offsets `0..every` together collect at every one once):
/// `LUNA_STRESS_SLOW=<dialect>/<file>` names a fixture the slow list
/// holds, `LUNA_JIT_HOT` the threshold. The output it is held to is the
/// interpreter's with no collections forced: the slow fixtures print
/// nothing the collector's timing changes.
#[cfg(feature = "gc-stress")]
#[test]
#[ignore = "slow: the stress-slow workflow runs it in shards"]
fn slow_fixture_shard() {
    use luna_core::__private::VmGcStress;
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} must be set"));
    let num = |k: &str| var(k).parse::<u64>().expect("a number");
    let which = var("LUNA_STRESS_SLOW");
    let (dialect, file) = which.split_once('/').expect("<dialect>/<file>");
    assert!(
        slow().iter().any(|(d, n, _)| d == dialect && n == file),
        "{which} is not on the slow list"
    );
    let &(_, version) = DIALECTS
        .iter()
        .find(|&&(d, _)| d == dialect)
        .expect("a dialect");
    let (every, offset, hot) = (
        num("LUNA_GC_STRESS_EVERY"),
        num("LUNA_GC_STRESS_OFFSET"),
        num("LUNA_JIT_HOT"),
    );
    assert!(
        HOT.contains(&(hot as u32)),
        "a hot threshold the corpus runs at"
    );
    let f = fixtures(dialect)
        .into_iter()
        .find(|p| p.ends_with(file))
        .expect("the fixture");
    let source = std::fs::read(&f).expect("read fixture");
    let mut interp_vm = super::trace_corpus::seeded(version, 1);
    interp_vm.set_jit_enabled(false);
    interp_vm.set_trace_jit_enabled(false);
    interp_vm.gc_stress_interval(u64::MAX, 0);
    let interp = masked(run(&mut interp_vm, &source));
    drop(interp_vm);
    let mut vm = super::trace_corpus::seeded(version, 1);
    vm.jit_mut().trace_hot_threshold = hot as u32;
    vm.jit_mut().call_hot_threshold = hot as u32;
    vm.gc_stress_interval(every, offset);
    vm.gc_stress_log();
    let t = std::time::Instant::now();
    let jit = masked(run(&mut vm, &source));
    eprintln!(
        "[forced_jit_corpus] {which} hot {hot}, every {every}th checkpoint from the {offset}th: \
         {} collections, {:.1}s",
        vm.gc_stress_collected().len(),
        t.elapsed().as_secs_f64()
    );
    assert!(
        !vm.gc_stress_collected().is_empty(),
        "no checkpoint collected"
    );
    assert_eq!(jit, interp, "{which} (hot = {hot}) under the JIT");
}
