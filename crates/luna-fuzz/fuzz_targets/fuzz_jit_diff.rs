//! JIT vs interpreter differential fuzz target.
//!
//! Generates programs meant to get hot (bounded loops, recursion,
//! closures, metatables, NaN / inf / -0, integer overflow, varargs) whose
//! operand types and metatables change while traces run, then runs each
//! twice in-process: with the JIT on and hot thresholds lowered (the
//! method and trace JIT together for half the inputs, each alone for a
//! quarter), and with the JIT off. The captured `print` output and the
//! error message, if any, must be identical.
//!
//! `$LUNA_FUZZ_DIALECT` (`5.1` … `5.5`, default `5.5`) picks the dialect.
//! `$LUNA_FUZZ_JIT_STATS=<file>` appends, every 64 inputs, a line
//! `inputs compiled dispatched errored skipped` (counts since the last
//! line) so a campaign can report how many inputs reached a trace.
//! `$LUNA_FUZZ_SHOW=1` prints each program and both results.
//! `$LUNA_FUZZ_JIT=<tiers>,<trace hot>,<call hot>` (tiers `both`, `trace`
//! or `method`) replaces the JIT setup read from the input, so one saved
//! input can be replayed under every setup; the program stays the same.
//!
//! Run:
//!     cd crates/luna-fuzz
//!     cargo +nightly fuzz run fuzz_jit_diff --fuzz-dir . -- -max_total_time=300

#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use luna_core::runtime::Value;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

#[allow(dead_code)]
#[path = "program.rs"]
mod program;

#[path = "jit_program.rs"]
mod jit_program;

#[path = "jit_stmts.rs"]
mod jit_stmts;

#[path = "jit_hot.rs"]
mod jit_hot;

struct Outcome {
    out: String,
    err: Option<String>,
    compiled: u64,
    dispatched: u64,
    /// trace recording / compile counters, shown with `$LUNA_FUZZ_SHOW`
    detail: String,
}

/// Which JIT tiers run. The method JIT compiles a function on its first
/// call, so with both on a loop inside a function traces only where the
/// method JIT declined it; some inputs run each tier alone.
#[derive(Clone, Copy, Debug)]
enum Tiers {
    Both,
    TraceOnly,
    MethodOnly,
}

/// `jit`: the tiers and the trace / call hot thresholds, `None` for the
/// interpreter.
fn run(src: &str, jit: Option<(Tiers, u32, u32)>) -> Outcome {
    let mut vm = luna_jit::new_with_jit(program::dialect());
    match jit {
        Some((tiers, trace, call)) => {
            vm.set_jit_enabled(!matches!(tiers, Tiers::TraceOnly));
            vm.set_trace_jit_enabled(!matches!(tiers, Tiers::MethodOnly));
            vm.jit.trace_hot_threshold = trace;
            vm.jit.call_hot_threshold = call;
        }
        None => {
            vm.set_jit_enabled(false);
            vm.set_trace_jit_enabled(false);
        }
    }
    vm.set_memory_cap(Some(64 << 20));
    let f = match vm.load(src.as_bytes(), b"=prog") {
        Ok(f) => f,
        Err(e) => panic!(
            "generated program does not load: {}\n{src}",
            String::from_utf8_lossy(&e.msg)
        ),
    };
    let err = match vm.call_value(Value::Closure(f), &[]) {
        Ok(_) => None,
        Err(e) => Some(mask_addresses(&vm.error_display(&e))),
    };
    vm.set_memory_cap(None);
    let out = match vm.eval("return table.concat(__out, '\\n')") {
        Ok(r) => match r.first() {
            Some(Value::Str(s)) => mask_addresses(&String::from_utf8_lossy(s.as_bytes())),
            other => panic!("capture buffer is {other:?}"),
        },
        Err(e) => panic!("reading the capture buffer: {}", vm.error_text(&e)),
    };
    Outcome {
        out,
        err,
        compiled: vm.trace_compiled_count(),
        dispatched: vm.trace_dispatched_count(),
        detail: format!(
            "closed {} aborted {} compile-failed {} deopts {} causes {:?}",
            vm.trace_closed_count(),
            vm.trace_aborted_count(),
            vm.trace_compile_failed_count(),
            vm.trace_deopt_count(),
            vm.trace_close_cause_counts()
        ),
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

/// The memory cap is checked between dispatch turns, which compiled code
/// does not take, so where it fires is not comparable.
fn hit_memory_cap(o: &Outcome) -> bool {
    let cap = "memory cap exceeded";
    o.out.contains(cap) || o.err.as_deref().is_some_and(|e| e.contains(cap))
}

/// `$LUNA_FUZZ_JIT`, parsed once.
fn jit_override() -> Option<(Tiers, u32, u32)> {
    static PARSED: std::sync::OnceLock<Option<(Tiers, u32, u32)>> = std::sync::OnceLock::new();
    *PARSED.get_or_init(|| {
        let v = std::env::var("LUNA_FUZZ_JIT").ok()?;
        let parts: Vec<&str> = v.split(',').collect();
        let bad = || panic!("LUNA_FUZZ_JIT={v:?}: want <both|trace|method>,<trace hot>,<call hot>");
        let [tiers, trace, call] = parts[..] else {
            bad()
        };
        let tiers = match tiers {
            "both" => Tiers::Both,
            "trace" => Tiers::TraceOnly,
            "method" => Tiers::MethodOnly,
            _ => bad(),
        };
        let hot = |s: &str| s.parse::<u32>().unwrap_or_else(|_| bad());
        Some((tiers, hot(trace), hot(call)))
    })
}

static INPUTS: AtomicU64 = AtomicU64::new(0);
static COMPILED: AtomicU64 = AtomicU64::new(0);
static DISPATCHED: AtomicU64 = AtomicU64::new(0);
static ERRORED: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);

fn record(jit: Option<&Outcome>) {
    match jit {
        Some(o) => {
            COMPILED.fetch_add((o.compiled > 0) as u64, Ordering::Relaxed);
            DISPATCHED.fetch_add((o.dispatched > 0) as u64, Ordering::Relaxed);
            ERRORED.fetch_add(o.err.is_some() as u64, Ordering::Relaxed);
        }
        None => {
            SKIPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
    if INPUTS.fetch_add(1, Ordering::Relaxed) % 64 != 63 {
        return;
    }
    let Ok(path) = std::env::var("LUNA_FUZZ_JIT_STATS") else {
        return;
    };
    let line = format!(
        "{} {} {} {} {}\n",
        INPUTS.swap(0, Ordering::Relaxed),
        COMPILED.swap(0, Ordering::Relaxed),
        DISPATCHED.swap(0, Ordering::Relaxed),
        ERRORED.swap(0, Ordering::Relaxed),
        SKIPPED.swap(0, Ordering::Relaxed),
    );
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("open {path}: {e}"));
    f.write_all(line.as_bytes())
        .unwrap_or_else(|e| panic!("write {path}: {e}"));
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    const HOT: [u32; 5] = [1, 2, 3, 7, 16];
    let trace = HOT[u.int_in_range(0..=4).unwrap_or(0) as usize];
    let call = HOT[u.int_in_range(0..=4).unwrap_or(0) as usize];
    let tiers = match u.int_in_range(0..=3).unwrap_or(0) {
        0 | 1 => Tiers::Both,
        2 => Tiers::TraceOnly,
        _ => Tiers::MethodOnly,
    };
    let src = jit_program::Gen::new(&mut u, program::dialect()).program();
    let (tiers, trace, call) = jit_override().unwrap_or((tiers, trace, call));

    let show = std::env::var_os("LUNA_FUZZ_SHOW").is_some();
    if show {
        eprintln!("=== source ({tiers:?}, trace hot {trace}, call hot {call}) ===\n{src}");
    }
    let interp = run(&src, None);
    if show {
        eprintln!("=== interpreter finished ===");
    }
    let jit = run(&src, Some((tiers, trace, call)));
    if show {
        eprintln!(
            "=== interpreter ===\n{}\nerror: {:?}\n=== jit ===\n{}\nerror: {:?}\ncompiled {} dispatched {} {}",
            interp.out, interp.err, jit.out, jit.err, jit.compiled, jit.dispatched, jit.detail
        );
    }
    if hit_memory_cap(&interp) || hit_memory_cap(&jit) {
        record(None);
        return;
    }
    record(Some(&jit));
    if (&interp.out, &interp.err) != (&jit.out, &jit.err) {
        panic!(
            "JIT differs from the interpreter ({tiers:?}, trace hot {trace}, call hot {call}, {} traces dispatched)\n\
             === source ===\n{src}\n=== interpreter ===\n{}\nerror: {:?}\n=== jit ===\n{}\nerror: {:?}",
            jit.dispatched, interp.out, interp.err, jit.out, jit.err
        );
    }
});
