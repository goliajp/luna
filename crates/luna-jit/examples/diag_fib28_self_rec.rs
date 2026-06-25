//! v2.0 Track-R prep diagnostic — measures luna trace JIT engagement
//! on the canonical self-recursive workload `fib(28)`. This is
//! research instrumentation only; no product code path depends on it.
//!
//! Run: `cargo run --example diag_fib28_self_rec --release -p luna-jit`

use luna_jit::version::LuaVersion;

const FIB_SRC: &str = r#"
    local function fib(n)
        if n < 2 then return n end
        return fib(n - 1) + fib(n - 2)
    end
    return fib(28)
"#;

const EXPECTED_FIB_28: i64 = 317_811;

fn run(label: &str, p16_on: bool) {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
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
    println!("== {label} (p16_self_link_enabled = {p16_on}) ==");
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
    println!();
}

fn main() {
    println!("# Track-R prep diag — fib(28) self-recursion JIT engagement");
    run("ship default", false);
    run("audit-flagged WRONG path", true);
    println!("# Reference shape:");
    println!("#   LuaJIT 2.1 fib(28) compiles ~12-mcode-instr loop body");
    println!("#   (rec_call_setup detects self-tail-call,");
    println!("#    asm_tail_link emits mcode back-branch).");
}
