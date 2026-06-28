//! v2.1 Path D Phase 1G.C.5 — production fire-count probe.
//!
//! Drives the same token_bucket_1k workload as
//! `bench_a4_prime_token_bucket`, then reads the cross-block DSE fire
//! counters from `cranelift_codegen::cross_block_dse_fire_count()`
//! (Phase 1G.C.2 added these counters to the vendored cranelift fork).
//!
//! Critical signal:
//!   - `invocations == 0`: nothing reached the cross-block branch on
//!     any compiled trace. Either the dual-write hasn't moved the
//!     surface OR the production traces never see the
//!     `prior_block != current_block && block_dominates(prior, cur)`
//!     shape on the migrated MemoryLocs. Either way, sub-2A re-apply
//!     gains nothing from Phase 1G.B — Phase 1H broadening needed.
//!   - `invocations > 0 && accepts == 0`: the cross-block branch was
//!     considered but never accepted (strict-chain rejected by
//!     can_trap intervening or off-chain successors not deopt-safe).
//!     This is the R1 risk Phase 1G.A flagged: relaxation needs to
//!     extend.
//!   - `invocations > 0 && accepts > 0`: rule fires on production
//!     trace shape — proceed to bench gate (Phase 1G.C.6).
//!
//! ### Run
//!
//! ```sh
//! cargo run --example probe_phase_1g_c_fire_count -p luna-jit --release
//! ```
//!
//! Exit code 0 always (probe is observational, not a regression
//! test). The counter values are printed; the verdict text is
//! advisory.

use std::hint::black_box;

use cranelift_codegen::{cross_block_dse_fire_count, reset_cross_block_dse_fire_count};
use luna_jit::new_minimal_with_jit;
use luna_jit::version::LuaVersion;

const SRC: &str = r#"
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

fn one_run() {
    let mut vm = new_minimal_with_jit(LuaVersion::Lua54);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    let _ = black_box(vm.eval(SRC).expect("eval"));
}

fn main() {
    // Reset counters so prior process-level work doesn't pollute.
    reset_cross_block_dse_fire_count();
    let (inv0, acc0) = cross_block_dse_fire_count();
    eprintln!("# baseline (after reset): invocations={inv0} accepts={acc0}");

    // Drive 5 compile + run cycles so trace-recorder + cranelift
    // optimisation see real multi-block bodies.
    for run_idx in 0..5 {
        one_run();
        let (inv, acc) = cross_block_dse_fire_count();
        eprintln!("# after run {run_idx}: invocations={inv} accepts={acc}");
    }

    let (invocations, accepts) = cross_block_dse_fire_count();
    println!();
    println!("# v2.1 Path D Phase 1G.C.5 fire-count verdict");
    println!("invocations = {invocations}");
    println!("accepts     = {accepts}");
    if invocations == 0 {
        println!(
            "# R1 RISK CONFIRMED: cross-block branch never entered on token_bucket_1k.\n\
             # → strict-chain + deopt-safe rule does NOT fire on production trace shape.\n\
             # → Phase 1G.C must REVERT sub-2A re-apply; handoff to Phase 1H broader rule."
        );
    } else if accepts == 0 {
        println!(
            "# R1 RISK PARTIAL: cross-block considered {invocations} times, accepted 0.\n\
             # → strict-chain / deopt-safe checks all rejected.\n\
             # → Phase 1G.C must REVERT sub-2A re-apply; handoff to Phase 1H broader rule."
        );
    } else {
        let ratio = (accepts as f64) / (invocations as f64) * 100.0;
        println!(
            "# RULE FIRES: {accepts}/{invocations} cross-block invocations accepted ({ratio:.1}%).\n\
             # → proceed to Phase 1G.C.6 bench gate (15 process reruns × 30 samples)."
        );
    }
}
