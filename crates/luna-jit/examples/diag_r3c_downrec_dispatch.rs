//! v2.0 Track-R R3c — diagnostic for the dispatcher's downrec-admit
//! arm. Reports `downrec_dispatched` (caller-pc guard HIT) vs
//! `downrec_deopt` (guard MISS or pending_err on a downrec entry).
//! Run:
//!
//! ```text
//! cargo run --example diag_r3c_downrec_dispatch --release -p luna-jit
//! ```
//!
//! Reference baseline (R3c ship, aarch64 macOS, fib(3) hot loop ×200):
//! ```text
//! returned:              Some(Int(400))
//! downrec_link_compiled: 1
//! downrec_dispatched:    93
//! downrec_deopt:         837
//! close_cause_counts:    {"downrec-stitch-pending": 1,
//!                         "partial-coverage-discard": 1,
//!                         "downrec-restart": 2}
//! ```
//!
//! Miss-rate = 837 / (93 + 837) = 90.0%. R3 prep §7.1 mitigation
//! gate ships at < 10%; R3c's single-CMP guard (parent frame's pc
//! vs IR-baked `dr_return_pc`) is above that, so R3d's lift to
//! `dispatchable=true` will need a multi-way guard (one CMP per
//! distinct `caller_pc` recorded in `rec.retfs`) before the perf
//! gain materialises.

use luna_jit::version::LuaVersion;
fn main() {
    let src = b"
        local function fib(n)
            if n < 2 then return n end
            return fib(n - 1) + fib(n - 2)
        end
        local s = 0
        for i = 1, 200 do s = s + fib(3) end
        return s
    ";
    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);
    vm.set_p16_self_link_enabled(true);
    vm.open_base();
    let cl = vm.load(src, b"=fib3_loop_r3c_diag").unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    println!("returned:              {:?}", r.first());
    println!(
        "downrec_link_compiled: {}",
        vm.trace_downrec_link_compiled_count()
    );
    println!(
        "downrec_dispatched:    {}",
        vm.trace_downrec_dispatched_count()
    );
    println!("downrec_deopt:         {}", vm.trace_downrec_deopt_count());
    println!("close_cause_counts:    {:?}", vm.trace_close_cause_counts());
}
