//! v2.0 Track-R R3d — perf gain measurement for the multi-way guard
//! + `dispatchable=true` lift.
//!
//! Reports wall-clock time for the fib(3) hot loop with p16-on vs
//! p16-off, plus the dispatched/deopt classification breakdown. R3c
//! shipped with p16-on showing a 10% hit-rate; R3d's multi-way fan-
//! out + lift target is to invert that into hot-loop dispatched
//! gains. fib(28) is NOT the right workload here because it closes
//! via SelfLink BEFORE DownRec can fire (see `diag_fib28_self_rec`
//! Row 3 docstring); fib(3) hot loop exposes the DownRec path.
//!
//! Audit Top-1 attack target (R3 prep §4.1) = ~22 ms gain on
//! fib(28). Realistic R3d-alone target = any positive delta on the
//! workload where DownRec actually fires (fib(3) hot loop). This
//! diag's role is to surface the numbers for the verdict doc;
//! it's not a formal bench (no statistical rigor — single-shot
//! timing per perf-methodology §6 FAQ).
//!
//! Run: `cargo run --example diag_r3d_fib3_gain --release -p luna-jit`

use luna_jit::version::LuaVersion;

const FIB_3_HOT_LOOP_TEMPLATE: &str = r#"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    local s = 0
    for i = 1, ITERS do s = s + fib(3) end
    return s
"#;

fn run(label: &str, p16_on: bool, iters: u64) -> std::time::Duration {
    let src = FIB_3_HOT_LOOP_TEMPLATE.replace("ITERS", &iters.to_string());
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(p16_on);
    vm.open_base();
    let cl = vm
        .load(src.as_bytes(), b"=fib3_loop_r3d_gain")
        .expect("loads");
    let t0 = std::time::Instant::now();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .expect("runs");
    let elapsed = t0.elapsed();
    let returned = match r.first() {
        Some(luna_jit::runtime::Value::Int(i)) => *i,
        Some(luna_jit::runtime::Value::Float(f)) => *f as i64,
        _ => -1,
    };
    let expected = (iters * 2) as i64;
    println!(
        "== {label} (p16={p16_on}, iters={iters}) ==\n  \
         returned: {returned}  expected: {expected}  \
         correct: {}\n  elapsed: {:?}",
        returned == expected,
        elapsed
    );
    println!(
        "  trace_dispatched:       {}",
        vm.trace_dispatched_count()
    );
    println!("  trace_deopt:            {}", vm.trace_deopt_count());
    println!(
        "  downrec_link_compiled:  {}",
        vm.trace_downrec_link_compiled_count()
    );
    println!(
        "  downrec_dispatched:     {}",
        vm.trace_downrec_dispatched_count()
    );
    println!(
        "  downrec_deopt:          {}",
        vm.trace_downrec_deopt_count()
    );
    println!(
        "  multi_way_guard_emitted:{}",
        vm.trace_multi_way_guard_emitted_count()
    );
    println!();
    elapsed
}

fn main() {
    const ITERS: u64 = 10_000;
    println!(
        "# fib(3) hot loop ×{ITERS} — R3d perf delta on the DownRec path.\n\
         # Single-shot wall-clock, NOT a statistical bench.\n"
    );
    // Two p16-off runs to warm CPU caches; report the second.
    let _warmup = run("warmup p16-off", false, ITERS);
    let off = run("p16-off baseline", false, ITERS);
    let on = run("p16-on  (R3d path)", true, ITERS);
    let delta = if on < off {
        format!("FASTER by {:?}", off - on)
    } else {
        format!("SLOWER by {:?}", on - off)
    };
    println!("# Delta (p16-on vs p16-off): {delta}");
    println!(
        "# R3c baseline (per R3c verdict §3): \
         on a similar fib(3) hot loop the dispatch hit-rate was 10% \
         and ~837 of 930 admits classified as downrec_deopt — \
         indicating the trace ran but the guard mostly missed.\n\
         # R3d target: dispatched > deopt + observable wall-clock \
         improvement vs R3c. The realistic A1 µs gain audit value \
         (~22 ms on fib(28)) doesn't apply directly here because \
         fib(3) hot loop's per-iter cost is dominated by the \
         200-iter for-loop control flow + interp; R3d's actual gain \
         shows up as 'p16-on no longer regresses' relative to \
         p16-off.",
    );
}
