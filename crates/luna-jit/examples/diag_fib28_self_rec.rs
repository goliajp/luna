//! v2.0 Track-R prep diagnostic — measures luna trace JIT engagement
//! on the canonical self-recursive workload `fib(28)`. This is
//! research instrumentation only; no product code path depends on it.
//!
//! Run: `cargo run --example diag_fib28_self_rec --release -p luna-jit`
//!
//! ## R0 finding (2026-06-26)
//!
//! With both `jit.enabled` (chunk-compiler / int-arith call-op JIT,
//! `try_jit_call_op` at `exec.rs:1519`) AND `jit.trace_enabled` on,
//! the trace recorder NEVER fires on `fib(28)`: the chunk-compiler
//! JIT compiles fib's body (int-arith whitelist hits), and every
//! recursive call short-circuits through `try_jit_call_op` BEFORE
//! reaching `push_frame` — the site that bumps
//! `Proto.call_hot_count` (`exec.rs:3839`). All trace counters stay
//! at zero.
//!
//! Disabling the chunk-compiler JIT (`vm.set_jit_enabled(false)`)
//! routes calls through the interpreter, the call-trigger hot
//! counter ticks past `CALL_HOT_THRESHOLD`, and the recorder
//! engages. With `p16_self_link_enabled = false` (ship default),
//! trace_dispatched_count = 434k+ on fib(28) — the audit-referenced
//! self-recursion downrec dispatcher figure. With p16 on, the
//! self-link cycle catch fires (`returned = 45 ≠ 317811`),
//! reproducing the documented WRONG path that gates p16's RFC.

use luna_jit::version::LuaVersion;

const FIB_SRC: &str = r#"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
"#;

const EXPECTED_FIB_28: i64 = 317_811;

/// One probe row. `chunk_jit` toggles the int-arith call-op JIT
/// (`Vm::set_jit_enabled`). The trace JIT itself stays on for every
/// row — we want to see whether the recorder can engage.
fn run(label: &str, chunk_jit: bool, p16_on: bool) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    // R0 mechanism: turning OFF `jit.enabled` is the surgical knob
    // that lets the trace recorder see fib's call sites. When ON,
    // `try_jit_call_op` short-circuits before `push_frame` and the
    // call-trigger hot counter is never bumped.
    vm.set_jit_enabled(chunk_jit);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(p16_on);
    vm.open_base();
    vm.open_math();
    let r = vm.eval(FIB_SRC).expect("eval failed");
    let returned = r
        .first()
        .and_then(|v| match v {
            luna_jit::runtime::Value::Int(i) => Some(*i),
            luna_jit::runtime::Value::Float(f) => Some(*f as i64),
            _ => None,
        })
        .unwrap_or(-1);
    println!("== {label} (chunk_jit = {chunk_jit}, p16 = {p16_on}) ==");
    println!("  returned:               {returned}");
    println!("  expected:               {EXPECTED_FIB_28}");
    println!("  result_correct:         {}", returned == EXPECTED_FIB_28);
    println!("  trace_closed:           {}", vm.trace_closed_count());
    println!("  trace_aborted:          {}", vm.trace_aborted_count());
    println!("  trace_compiled:         {}", vm.trace_compiled_count());
    println!(
        "  trace_compile_failed:   {}",
        vm.trace_compile_failed_count()
    );
    println!("  trace_dispatched:       {}", vm.trace_dispatched_count());
    println!("  trace_deopt:            {}", vm.trace_deopt_count());
    println!(
        "  side_trace_compiled:    {}",
        vm.trace_side_trace_compiled_count()
    );
    println!("  trace_max_depth_seen:   {}", vm.trace_max_depth_seen());

    if !vm.trace_compile_failed_reasons().is_empty() {
        println!("  compile_failed_reasons:");
        for r in vm.trace_compile_failed_reasons() {
            println!("    - {}", r);
        }
    }
    if !vm.trace_closed_lens().is_empty() {
        println!("  closed_lens (n = {}):", vm.trace_closed_lens().len());
        for (is_call, n) in vm.trace_closed_lens() {
            println!("    - is_call_triggered = {}, ops_len = {}", is_call, n);
        }
    }
    if !vm.trace_dispatch_off_reasons().is_empty() {
        let mut by_reason: std::collections::BTreeMap<&str, u64> =
            std::collections::BTreeMap::new();
        for r in vm.trace_dispatch_off_reasons() {
            *by_reason.entry(*r).or_insert(0) += 1;
        }
        println!("  dispatch_off_reasons (top):");
        for (r, c) in by_reason.iter().take(8) {
            println!("    - {} : {}", r, c);
        }
    }
    // v2.0 Track-R R2 — surface the close-cause HashMap so probes can
    // diff against the ordered Vec above. Recorder-side reasons
    // (`trace-overflow`, `partial-coverage-discard`) only show up
    // here; lowerer-side reasons mirror both surfaces.
    let close_cause = vm.trace_close_cause_counts();
    if !close_cause.is_empty() {
        // BTreeMap for deterministic diag output ordering.
        let sorted: std::collections::BTreeMap<&str, u64> =
            close_cause.iter().map(|(k, v)| (*k, *v)).collect();
        println!("  close_cause_counts (R2, top):");
        for (r, c) in sorted.iter().take(8) {
            println!("    - {} : {}", r, c);
        }
    }
    println!();
}

fn main() {
    println!("# Track-R R0 diag — fib(28) self-recursion JIT engagement");
    println!("#");
    println!("# Row 1 reproduces the original audit observation: with the");
    println!("# chunk-compiler JIT on (ship default for embedders that take");
    println!("# `new_minimal_with_jit`), the trace recorder never sees fib's");
    println!("# call sites and every counter stays at zero.");
    println!("#");
    println!("# Rows 2 & 3 are the R0 fire mechanism: chunk-compiler JIT off,");
    println!("# trace JIT on. Row 2 = ship default p16 (off) — recorder engages,");
    println!("# trace_dispatched climbs into hundreds of thousands. Row 3 flips");
    println!("# p16_self_link_enabled — the result corrupts to 45, reproducing");
    println!("# the WRONG-path that gates the p16 RFC.");
    println!();
    run("R0 baseline — recorder swallowed by chunk JIT", true, false);
    run("R0 fire — chunk JIT off, ship default p16", false, false);
    run(
        "R0 fire — chunk JIT off, p16 self-link ON (WRONG)",
        false,
        true,
    );
    println!("# Reference shape:");
    println!("#   LuaJIT 2.1 fib(28) compiles ~12-mcode-instr loop body");
    println!("#   (rec_call_setup detects self-tail-call,");
    println!("#    asm_tail_link emits mcode back-branch).");
}
