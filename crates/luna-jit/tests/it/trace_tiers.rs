//! The two trace tiers against the interpreter: the baseline code
//! generator alone, Cranelift alone, and the baseline tier moving traces to
//! Cranelift (at once, or partway through a run). The tier is a per-Vm
//! setting, so the settings run side by side in one process.

use std::path::{Path, PathBuf};

use luna_jit::jit::trace::TraceTier;
use luna_jit::jit_backend::trace::{baseline_codegen_count, baseline_fallback};
use luna_jit::runtime::Value;
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

const DIALECTS: &[(&str, LuaVersion)] = &[
    ("5.1", LuaVersion::Lua51),
    ("5.2", LuaVersion::Lua52),
    ("5.3", LuaVersion::Lua53),
    ("5.4", LuaVersion::Lua54),
    ("5.5", LuaVersion::Lua55),
];

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

/// `source`'s output with `print` / `io.write` captured, or its error;
/// table and function addresses masked.
fn run(vm: &mut Vm, source: &[u8]) -> Result<String, String> {
    vm.eval(
        r#"
_G.__out = {}
function print(...)
    local t = {}
    for i = 1, select('#', ...) do t[i] = tostring(select(i, ...)) end
    _G.__out[#_G.__out + 1] = table.concat(t, '\t') .. '\n'
end
io.write = function(...)
    for i = 1, select('#', ...) do _G.__out[#_G.__out + 1] = tostring(select(i, ...)) end
end
"#,
    )
    .expect("capture preamble");
    let r = match vm.load(source, b"=t") {
        Err(e) => Err(String::from_utf8_lossy(&e.msg).into_owned()),
        Ok(f) => match vm.call_value(Value::Closure(f), &[]) {
            Err(e) => Err(vm.error_display(&e)),
            Ok(_) => match vm
                .eval("return table.concat(_G.__out)")
                .expect("buffer")
                .first()
            {
                Some(Value::Str(s)) => Ok(String::from_utf8_lossy(s.as_bytes()).into_owned()),
                other => panic!("expected the capture buffer, got {other:?}"),
            },
        },
    };
    r.map(|s| mask(&s)).map_err(|s| mask(&s))
}

fn mask(s: &str) -> String {
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

fn interp(version: LuaVersion) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(false);
    vm
}

/// A trace-JIT Vm recording loops after `hot` back edges, in `tier`;
/// `tier_up_at` for [`TraceTier::Auto`].
fn tiered(version: LuaVersion, hot: u32, tier: TraceTier, tier_up_at: u32) -> Vm {
    let mut vm = luna_jit::new_with_jit(version);
    vm.set_jit_enabled(false);
    vm.jit.trace_hot_threshold = hot;
    vm.jit.call_hot_threshold = hot;
    vm.set_trace_tier(tier);
    vm.set_trace_tier_up_at(tier_up_at);
    vm
}

/// What a corpus run did: trace entries, traces moved to Cranelift.
#[derive(Default)]
struct Seen {
    dispatched: u64,
    tiered_up: u64,
}

fn corpus(tier: TraceTier, tier_up_at: u32) -> Seen {
    let mut failed = Vec::new();
    let mut seen = Seen::default();
    for &(dialect, version) in DIALECTS {
        for f in &fixtures(dialect) {
            let source = std::fs::read(f).expect("read fixture");
            let want = run(&mut interp(version), &source);
            for hot in [1, 3] {
                let mut vm = tiered(version, hot, tier, tier_up_at);
                let got = run(&mut vm, &source);
                seen.dispatched += vm.trace_dispatched_count();
                seen.tiered_up += vm.trace_tiered_up_count();
                if got != want {
                    failed.push(format!(
                        "{} (hot {hot}):\n  interpreter: {want:?}\n  traces:      {got:?}",
                        f.display()
                    ));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(seen.dispatched > 0, "no trace ran, so nothing was compared");
    seen
}

#[test]
fn the_baseline_tier_runs_the_corpus_like_the_interpreter() {
    let before = baseline_codegen_count();
    let fallback = baseline_fallback().0;
    let seen = corpus(TraceTier::Baseline, 0);
    assert!(
        baseline_codegen_count() > before,
        "no baseline code was made"
    );
    assert_eq!(
        baseline_fallback().0,
        fallback,
        "a trace fell back to Cranelift: {}",
        baseline_fallback().1
    );
    assert_eq!(seen.tiered_up, 0);
}

#[test]
fn the_optimizing_tier_runs_the_corpus_like_the_interpreter() {
    let before = baseline_codegen_count();
    corpus(TraceTier::Optimizing, 0);
    assert_eq!(
        baseline_codegen_count(),
        before,
        "Optimizing made baseline code"
    );
}

#[test]
fn moving_every_trace_to_cranelift_at_once_keeps_the_results() {
    let seen = corpus(TraceTier::Auto, 1);
    assert!(seen.tiered_up > 0, "no trace moved to Cranelift");
}

#[test]
fn moving_traces_to_cranelift_partway_keeps_the_results() {
    let seen = corpus(TraceTier::Auto, 37);
    assert!(seen.tiered_up > 0, "no trace moved to Cranelift");
}

/// `src` returns a function; run it under the interpreter and under `vm`
/// `calls` times, comparing every result.
fn compare_calls(vm: &mut Vm, src: &str, calls: usize) {
    let mut it = interp(LuaVersion::Lua54);
    let get = |vm: &mut Vm| {
        let main = vm.load(src.as_bytes(), b"=t").expect("load");
        match vm.call_value(Value::Closure(main), &[]).expect("chunk")[0] {
            Value::Closure(f) => f,
            ref v => panic!("chunk returned {v:?}"),
        }
    };
    let (f, g) = (get(vm), get(&mut it));
    for k in 0..calls {
        let a = vm.call_value(Value::Closure(f), &[]).expect("trace run");
        let b = it
            .call_value(Value::Closure(g), &[])
            .expect("interpreter run");
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "call {k}");
    }
}

const COUNTING_LOOP: &str = r#"
return function()
    local t = {n = 0, s = 0.5}
    for i = 1, 20000 do
        t.n = t.n + i % 7
        if i % 3 == 0 then t.s = t.s + 1.25 end
    end
    return t.n, t.s
end
"#;

#[test]
fn a_loop_moves_to_cranelift_partway_through_one_call() {
    let mut vm = tiered(LuaVersion::Lua54, 8, TraceTier::Auto, 1000);
    compare_calls(&mut vm, COUNTING_LOOP, 3);
    assert!(vm.trace_dispatched_count() > 0);
    assert!(
        vm.trace_tiered_up_count() > 0,
        "the loop stayed in the baseline tier"
    );
}

/// A generic-for trace over string keys that leaves at its head to move
/// to Cranelift: the interpreter must get the key back as a string, or
/// `next` rejects it.
#[test]
fn a_generic_for_left_at_its_head_hands_back_the_key() {
    let src = r#"
return function()
    local t = {}
    for i = 1, 200 do t["k" .. i] = i end
    local n, s = 0, 0
    for k, v in pairs(t) do
        n = n + 1
        s = s + v
    end
    return n, s
end
"#;
    for at in [3, 5, 9, 17] {
        let mut vm = tiered(LuaVersion::Lua54, 2, TraceTier::Auto, at);
        compare_calls(&mut vm, src, 3);
        assert!(
            vm.trace_tiered_up_count() > 0,
            "at {at}: nothing moved to Cranelift"
        );
    }
}

/// Self-recursion: the trace's hot exits get side traces, recorded,
/// compiled and linked into the parent's exits (the only shape the trace
/// JIT links side traces for).
const SIDE_EXITS: &str = r#"
local function f(n)
    if n < 2 then return n end
    return f(n - 1) + f(n - 2)
end
return function() return f(20) end
"#;

/// Tier-up counts on either side of the side exits' hot threshold, of the
/// side trace's recording and of its linking: the move to Cranelift lands
/// on a parent being linked and on a child just linked, and the results
/// stay the interpreter's.
#[test]
fn moving_to_cranelift_around_side_trace_linking_keeps_the_results() {
    let mut side = 0;
    let mut moved = 0;
    for at in (1..=60).chain([80, 120, 200, 400, 800, 3000]) {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.set_trace_tier(TraceTier::Auto);
        vm.set_trace_tier_up_at(at);
        compare_calls(&mut vm, SIDE_EXITS, 3);
        side += vm.trace_side_trace_compiled_count();
        moved += vm.trace_tiered_up_count();
    }
    assert!(side > 0, "no side trace was linked");
    assert!(moved > 0, "nothing moved to Cranelift");
}
