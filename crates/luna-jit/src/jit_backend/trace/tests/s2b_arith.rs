use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

/// Load a Lua chunk and return its outer Proto. The Vm must
/// outlive the returned `Gc<Proto>` — `Heap::drop` frees every
/// GC object regardless of reachability, so a `wide_proto()`
/// helper that owns its own Vm would hand back a dangling
/// pointer the moment it returns. Every test binds the Vm to
/// a local and threads it explicitly.
fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    let cl = vm.load(src, b"=t").expect("compile");
    cl.proto
}

/// Chunk source sized for ≥ 4 regs (`max_stack` = 5) — enough
/// for every test that touches R[0..=3].
const WIDE_SRC: &[u8] = b"local a,b,c,d = 0,0,0,0; return a+b+c+d";

fn make_record(head_pc: u32, ops: &[Inst], proto: Gc<Proto>) -> TraceRecord {
    // The registers these traces compute on hold integers.
    let tags = vec![luna_core::runtime::value::raw::INT; proto.max_stack as usize];
    let mut rec = TraceRecord::start(proto, head_pc, tags, false);
    for (i, inst) in ops.iter().copied().enumerate() {
        let pushed = rec.push(RecordedOp {
            proto,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
        assert!(pushed, "test trace must fit MAX_TRACE_LEN");
    }
    rec.closed = true;
    rec
}

#[test]
fn closed_empty_trace_returns_head_pc_and_passes_regs_through() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let rec = make_record(7, &[], p);
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("empty closed trace must compile");
    assert_eq!(ct.head_pc, 7);
    assert_eq!(ct.n_ops, 0);

    let mut state: Vec<i64> = vec![100, 200, 300, 400];
    // Resize to head_proto.max_stack so the trace's full store-back
    // pass has somewhere to write. Extra slots default to 0.
    state.resize(p.max_stack as usize, 0);
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };

    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        7,
        "clean close returns head_pc"
    );
    // First four slots passed through untouched — load-then-store
    // pattern preserves the i64 payload.
    assert_eq!(state[0], 100);
    assert_eq!(state[1], 200);
    assert_eq!(state[2], 300);
    assert_eq!(state[3], 400);
}

#[test]
fn add_trace_computes_sum_into_dst_reg() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = R[1] + R[2]
    let add = Inst::iabc(Op::Add, 0, 1, 2, false);
    let rec = make_record(11, &[add], p);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Add trace must compile");
    assert_eq!(ct.head_pc, 11);
    assert_eq!(ct.n_ops, 1);

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 10;
    state[2] = 3;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };

    assert_eq!(crate::jit_backend::trace::exit_pc(r), 11);
    assert_eq!(state[0], 13, "10 + 3");
    assert_eq!(state[1], 10, "input untouched");
    assert_eq!(state[2], 3, "input untouched");
}

#[test]
fn chained_arith_threads_results_through_regs() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = R[0] + R[1]
    // R[0] = R[0] * R[2]
    // R[0] = R[0] - R[3]
    let prog = [
        Inst::iabc(Op::Add, 0, 0, 1, false),
        Inst::iabc(Op::Mul, 0, 0, 2, false),
        Inst::iabc(Op::Sub, 0, 0, 3, false),
    ];
    let rec = make_record(0, &prog, p);
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Add/Mul/Sub chain must compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 5;
    state[1] = 3;
    state[2] = 4;
    state[3] = 7;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };

    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0, "head_pc 0");
    // ((5 + 3) * 4) - 7 = 25
    assert_eq!(state[0], 25);
}

#[test]
fn move_then_mul_propagates_via_move() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = R[3]      (Move)
    // R[0] = R[0] * R[1]
    let prog = [
        Inst::iabc(Op::Move, 0, 3, 0, false),
        Inst::iabc(Op::Mul, 0, 0, 1, false),
    ];
    let rec = make_record(0, &prog, p);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Move + Mul must compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 999; // overwritten by Move
    state[1] = 6;
    state[3] = 7;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };

    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
    assert_eq!(state[0], 42, "7 * 6");
}

#[test]
fn trailing_jmp_is_emit_no_op() {
    // The trace recorder typically pushes the back-edge Op::Jmp
    // as the last op before the close detection fires on the
    // next head_pc visit. Step 2 treats it as a no-op — the
    // tail (return head_pc) carries the control transfer.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Add, 0, 1, 2, false),
        // Use the unconditional Jmp shape; offset is irrelevant
        // to the lowerer (we know head_pc from the record).
        Inst::isj(Op::Jmp, -3),
    ];
    let rec = make_record(0, &prog, p);
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Add + trailing Jmp must compile");
    assert_eq!(ct.n_ops, 2);

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 4;
    state[2] = 5;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
    assert_eq!(state[0], 9);
}

#[test]
fn non_closed_trace_does_not_compile() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn unsupported_op_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // Op::Concat is not in the whitelist (and unlike Op::Return0,
    // which is a truncation point, Concat
    // has no special treatment and falls through to a bail).
    let prog = [Inst::iabc(Op::Concat, 0, 0, 0, false)];
    let rec = make_record(0, &prog, p);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn inline_depth_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let mut rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    rec.push(RecordedOp {
        proto: p,
        pc: 0,
        inst: Inst::iabc(Op::Add, 0, 1, 2, false),
        inline_depth: 1, // S4 territory — step 2 must bail.
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn cross_proto_op_bails() {
    let mut vm1 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let mut vm2 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p1 = load_proto(&mut vm1, WIDE_SRC);
    let p2 = load_proto(&mut vm2, WIDE_SRC);
    // Distinct `Vm::new` runs → distinct Proto allocations,
    // even though both compile from the same source. The
    // lowerer must reject any cross-Proto op.
    let mut rec = TraceRecord::start(
        p1,
        0,
        vec![luna_core::runtime::value::raw::INT; p1.max_stack as usize],
        false,
    );
    rec.push(RecordedOp {
        proto: p2,
        pc: 0,
        inst: Inst::iabc(Op::Add, 0, 1, 2, false),
        inline_depth: 0,
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm1.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn out_of_bounds_reg_bails() {
    // `return 0` compiles to a tiny chunk with max_stack = 2
    // (proven by `cargo run --example probe_ms`); R[2] in an
    // arith op lies at the boundary (index 2 == len) and must
    // be rejected.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, b"return 0");
    let prog = [Inst::iabc(Op::Add, 0, 1, 2, false)];
    let rec = make_record(0, &prog, p);
    // Guard the assertion so a future compiler change to
    // small-chunk max_stack doesn't silently turn the test
    // green via the loose-precondition path.
    assert!((p.max_stack as usize) <= 2, "precondition for this test");
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn compiled_trace_is_callable_repeatedly() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [Inst::iabc(Op::Add, 0, 1, 2, false)];
    let rec = make_record(0, &prog, p);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // Re-entrancy / mmap-lifetime sanity: each call recomputes
    // R[0] from the inputs, and the fn ptr stays valid because
    // `storage.trace_handles` keeps `JITModule` alive.
    for k in 0..1000 {
        state[1] = k;
        state[2] = 2 * k;
        let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
        assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
        assert_eq!(state[0], 3 * k);
    }
}
