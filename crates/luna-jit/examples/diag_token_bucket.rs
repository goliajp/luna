//! v1.2 P3a diagnostic — answers the P-A2 audit's "trace JIT engagement"
//! question for the `token_bucket_1k` workload.
//!
//! Runs the same workload as `benches/redis_lua_shape.rs` `token_bucket_1k`,
//! N=200 iterations on fresh Vms, then prints a counter snapshot accumulated
//! across all iters. The counters that matter for P-A2 budget reconciliation:
//!
//! - `trace_dispatched_count`: > 0 means trace JIT is engaged → the
//!   decomposition's stage costs apply to the trace path.
//! - `trace_dispatched_count == 0`: the workload runs interp-only → the
//!   decomposition must redo against the interp dispatcher, not the trace
//!   recorder/lowerer.
//! - `trace_compile_failed_reasons` (if compile_failed > 0): tells us
//!   WHY the compiler bailed — most relevant if recording succeeds but
//!   compile rejects (audit's hypothesis: `trace.rs:2097-2099` GetField
//!   gate aborts mid-recording for table-heavy workloads).
//! - `trace_closed_lens`: per-trace `(is_call_triggered, ops_len)` — gives
//!   us the trace shape distribution.
//!
//! Run: `cargo run --example diag_token_bucket --release`

use luna_jit::version::LuaVersion;

const TOKEN_BUCKET_SRC: &str = r#"
    local bucket = { tokens = 1000, last = 0, rate = 100 }
    local now = 1
    local refilled = 0
    for i = 1, 1000 do
        local elapsed = now - bucket.last
        local refill = elapsed * bucket.rate
        if refill > 0 then
            bucket.tokens = math.min(1000, bucket.tokens + refill)
            bucket.last = now
            refilled = refilled + 1
        end
        if bucket.tokens >= 1 then
            bucket.tokens = bucket.tokens - 1
        end
        now = now + 1
    end
    return bucket.tokens, refilled
"#;

const N_ITERS: usize = 200;

fn main() {
    // ── Run 1: fresh Vm per iter (matches the criterion harness) ─────────
    //
    // This is the "embedder-shape" diagnostic. Each iter is a cold Vm so
    // the trace JIT must warm up + record + compile within the 1k inner
    // iterations to engage. If the inner loop doesn't trigger a back-edge
    // hot enough to start recording, trace_recorded == 0.
    let mut acc_closed = 0u64;
    let mut acc_aborted = 0u64;
    let mut acc_compiled = 0u64;
    let mut acc_compile_failed = 0u64;
    let mut acc_dispatched = 0u64;
    let mut acc_deopt = 0u64;
    let mut all_compile_failed_reasons: Vec<String> = Vec::new();
    let mut all_closed_lens: Vec<(bool, usize)> = Vec::new();
    let mut all_dispatch_off: Vec<String> = Vec::new();

    // Trace JIT default is `false` (jit_state.rs:181). Diag enables it
    // explicitly so we can see whether the recorder + lowerer ENGAGE on
    // this workload at all — independent of whether v1.0/v1.1 shipped
    // with it on. If counters stay 0 with `trace_enabled = true`, the
    // workload's hot path doesn't trigger recording at all (no back-edge
    // hot enough, or unrecordable op aborts before the first close).
    for _ in 0..N_ITERS {
        let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
        vm.set_trace_jit_enabled(true);
        vm.open_base();
        vm.open_math();
        vm.open_string();
        vm.open_table();
        vm.eval(TOKEN_BUCKET_SRC)
            .expect("token_bucket script must run cleanly");

        acc_closed += vm.trace_closed_count();
        acc_aborted += vm.trace_aborted_count();
        acc_compiled += vm.trace_compiled_count();
        acc_compile_failed += vm.trace_compile_failed_count();
        acc_dispatched += vm.trace_dispatched_count();
        acc_deopt += vm.trace_deopt_count();
        for r in vm.trace_compile_failed_reasons() {
            all_compile_failed_reasons.push((*r).to_string());
        }
        for &(is_call, n) in vm.trace_closed_lens() {
            all_closed_lens.push((is_call, n));
        }
        for r in vm.trace_dispatch_off_reasons() {
            all_dispatch_off.push((*r).to_string());
        }
    }

    println!("# v1.2 P3a diag — token_bucket_1k trace JIT engagement");
    println!(
        "# {} fresh Vm iters; counters accumulated across all.",
        N_ITERS
    );
    println!();
    println!("trace lifecycle:");
    println!("  closed:            {}", acc_closed);
    println!("  aborted:           {}", acc_aborted);
    println!("  compiled:          {}", acc_compiled);
    println!("  compile_failed:    {}", acc_compile_failed);
    println!("  dispatched:        {}", acc_dispatched);
    println!("  deopt:             {}", acc_deopt);
    println!();
    println!("per-Vm averages:");
    println!(
        "  closed/Vm:         {:.3}",
        acc_closed as f64 / N_ITERS as f64
    );
    println!(
        "  compiled/Vm:       {:.3}",
        acc_compiled as f64 / N_ITERS as f64
    );
    println!(
        "  dispatched/Vm:     {:.3}",
        acc_dispatched as f64 / N_ITERS as f64
    );
    println!();

    // ── Histogram: closed lens ────────────────────────────────────────────
    if !all_closed_lens.is_empty() {
        println!("trace_closed_lens (is_call_triggered, ops_len) histogram:");
        let mut by_len: std::collections::BTreeMap<(bool, usize), u64> =
            std::collections::BTreeMap::new();
        for &k in &all_closed_lens {
            *by_len.entry(k).or_insert(0) += 1;
        }
        for ((is_call, n), count) in &by_len {
            println!("  ({:>5}, {:>4}) × {}", is_call, n, count);
        }
        println!();
    }

    // ── Compile failure reasons ──────────────────────────────────────────
    if !all_compile_failed_reasons.is_empty() {
        println!("compile_failed_reasons (top 10):");
        let mut by_reason: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for r in &all_compile_failed_reasons {
            *by_reason.entry(r.clone()).or_insert(0) += 1;
        }
        let mut sorted: Vec<_> = by_reason.into_iter().collect();
        sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (r, n) in sorted.into_iter().take(10) {
            println!("  {} × {}", r, n);
        }
        println!();
    }

    // ── Dispatch-off reasons ─────────────────────────────────────────────
    if !all_dispatch_off.is_empty() {
        println!("trace_dispatch_off_reasons (top 10):");
        let mut by_reason: std::collections::HashMap<String, u64> =
            std::collections::HashMap::new();
        for r in &all_dispatch_off {
            *by_reason.entry(r.clone()).or_insert(0) += 1;
        }
        let mut sorted: Vec<_> = by_reason.into_iter().collect();
        sorted.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (r, n) in sorted.into_iter().take(10) {
            println!("  {} × {}", r, n);
        }
        println!();
    }

    // ── Audit's hypothesis ───────────────────────────────────────────────
    println!("P-A2 audit hypothesis:");
    if acc_dispatched == 0 {
        println!("  ❌ trace_dispatched_count == 0 — workload runs INTERP-ONLY");
        println!("     P-A2 stage decomposition must be redone vs the interp dispatcher,");
        println!("     not the trace recorder + lowerer.");
    } else if acc_dispatched < acc_closed / 2 {
        println!(
            "  ⚠ trace_dispatched_count ({}) << trace_closed_count ({}) — most",
            acc_dispatched, acc_closed
        );
        println!("     traces don't get re-dispatched — short-lived hot paths.");
    } else {
        println!(
            "  ✅ trace_dispatched_count ({}) is non-trivial — trace JIT engages.",
            acc_dispatched
        );
        println!("     P-A2 stage costs apply to the trace path.");
    }
}
