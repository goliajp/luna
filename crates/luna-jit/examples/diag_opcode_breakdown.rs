//! v1.2 P3 Path A — opcode breakdown validation diag for the interp
//! dispatcher decomp at `.dev/rfcs/v1.2-audit-interp-decomp.md`.
//!
//! Per `~/.claude-shared/global/methodology/perf-decomposition-vs-polish.md`
//! §2 Phase A: "Decomposition 完成前必须 run 实际 workload 验证 high-level
//! 计数". The interp decomp's per-iter op mix is statically derived from
//! Lua 5.4 codegen knowledge — runtime validation closes the methodology
//! gap the audit's Open Questions §1 self-flagged.
//!
//! Counts every opcode dispatch on `token_bucket_1k` for a single Vm run.
//! Divides the total by 1000 (iters) to get per-iter mix; compares against
//! the audit's S01-S15 weights. Wide divergence (>10%) means the static
//! estimate is off and some stages' weights need rederiving.
//!
//! Run: `cargo run --example diag_opcode_breakdown --release`

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
    vm.open_base();
    vm.open_math();
    vm.open_string();
    vm.open_table();
    vm.set_rust_debug_hook(
        Some(tally_hook),
        luna_jit::vm::exec::HOOK_MASK_COUNT,
        1, // count_base = 1 → fire every instruction
    );
    vm.eval(TOKEN_BUCKET_SRC)
        .expect("token_bucket script must run cleanly");

    let snapshot: Vec<(Op, u64)> = COUNTS.with(|c| {
        let mut v: Vec<_> = c.borrow().clone();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        v
    });

    let total: u64 = snapshot.iter().map(|(_, n)| n).sum();
    // Token_bucket inner loop runs 1000 iters; the headline mix is per-iter.
    const INNER_ITERS: u64 = 1000;

    println!("# v1.2 P3 Path A — opcode breakdown for token_bucket_1k");
    println!(
        "# {} total dispatches across the whole script ({} per inner iter average).",
        total,
        total / INNER_ITERS
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
    println!();
    println!("# Audit's static estimate (from `.dev/rfcs/v1.2-audit-interp-decomp.md`):");
    println!("#   GetField: 5/iter, SetField: 2-3/iter, arith: 4-5/iter, Lt/Le: 2/iter,");
    println!("#   GetTabUp: 1/iter, Call: 1/iter, ForLoop: 1/iter, Move/LoadI: ~10/iter.");
    println!("# Divergence >10% on any of these means rederive S## weights and");
    println!("# redo budget reconciliation in the audit.");
}
