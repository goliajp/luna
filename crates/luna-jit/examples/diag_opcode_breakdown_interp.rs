//! v2.0 PI Phase 1 step 0 — opcode breakdown for token_bucket_1k
//! with the trace JIT DISABLED, so every inner-loop iteration goes
//! through the interp dispatcher. This is the ground-truth count
//! for the §5 18-stage decomposition; the existing
//! `diag_opcode_breakdown` runs with trace on (default), so it only
//! reports the pre-trace warmup dispatches and misses ~98% of the
//! iterations once the trace takes over.
//!
//! Run: `cargo run --release --example diag_opcode_breakdown_interp`

use std::cell::RefCell;

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;
use luna_jit::vm::isa::Op;

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

thread_local! {
    static COUNTS: RefCell<Vec<(Op, u64)>> = const { RefCell::new(Vec::new()) };
}

fn tally_hook(vm: &mut Vm, ev: luna_jit::vm::exec::RustHookEvent) {
    if !matches!(ev, luna_jit::vm::exec::RustHookEvent::Count) {
        return;
    }
    if let Some(op) = vm.current_op() {
        COUNTS.with(|c| {
            let mut v = c.borrow_mut();
            if let Some(entry) = v.iter_mut().find(|(o, _)| *o == op) {
                entry.1 += 1;
            } else {
                v.push((op, 1));
            }
        });
    }
}

fn main() {
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    // CRITICAL: disable trace so all inner-iter dispatches are interp.
    vm.set_trace_jit_enabled(false);
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    vm.set_rust_debug_hook(
        Some(tally_hook),
        luna_jit::vm::exec::HOOK_MASK_COUNT,
        1,
    );
    vm.eval(TOKEN_BUCKET_SRC)
        .expect("token_bucket script must run cleanly");

    let snapshot: Vec<(Op, u64)> = COUNTS.with(|c| {
        let mut v: Vec<_> = c.borrow().clone();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        v
    });

    let total: u64 = snapshot.iter().map(|(_, n)| n).sum();
    const INNER_ITERS: u64 = 1000;

    println!("# v2.0 PI Phase 1 step 0 — opcode breakdown (trace_jit=false)");
    println!(
        "# {} total dispatches across the whole script ({:.2} per inner iter).",
        total,
        total as f64 / INNER_ITERS as f64,
    );
    println!();
    println!(
        "{:>16} | {:>10} | {:>10} | {:>6}",
        "Op", "count", "per-iter", "share%"
    );
    println!(
        "{:>16} | {:>10} | {:>10} | {:>6}",
        "-".repeat(16),
        "-".repeat(10),
        "-".repeat(10),
        "-".repeat(6),
    );
    for (op, n) in &snapshot {
        let per_iter = *n as f64 / INNER_ITERS as f64;
        let share = (*n as f64 / total as f64) * 100.0;
        println!(
            "{:>16} | {:>10} | {:>10.3} | {:>5.1}%",
            format!("{:?}", op),
            n,
            per_iter,
            share
        );
    }
}
