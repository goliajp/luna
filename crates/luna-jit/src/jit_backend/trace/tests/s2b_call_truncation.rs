use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const WIDE_SRC: &[u8] = b"local a,b,c,d = 0,0,0,0; return a+b+c+d";

fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    let cl = vm.load(src, b"=t").expect("compile");
    cl.proto
}

fn closed_record(proto: Gc<Proto>, head_pc: u32, ops: &[Inst]) -> TraceRecord {
    let mut rec = TraceRecord::start(
        proto,
        head_pc,
        vec![luna_core::runtime::value::raw::INT; proto.max_stack as usize],
        false,
    );
    for (i, inst) in ops.iter().copied().enumerate() {
        let pushed = rec.push(RecordedOp {
            proto,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
        assert!(pushed);
    }
    rec.closed = true;
    rec
}

#[test]
fn single_call_op_side_exits_at_call_pc() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // A trace consisting of one Call at pc=0. Step-5 treats it
    // as a side-exit (no IR is emitted for the Call body).
    let rec = closed_record(
        p,
        5,
        &[Inst::iabc(Op::Call, 0, 1, 1, false)], // call R[0], 0 args, 0 results
    );
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Op::Call-only trace must compile");

    let mut state: Vec<i64> = vec![42, 43, 44, 0, 0];
    state.resize(p.max_stack as usize, 0);
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // Trace's "head_pc" was 5 — but the side-exit at the Call
    // returns the Call's PC (= 0 in this 1-op trace).
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        0,
        "side-exit at call's PC, not head_pc"
    );
    // Reg state passes through (we loaded + stored every reg).
    assert_eq!(state[0], 42);
    assert_eq!(state[1], 43);
}

#[test]
fn arith_then_call_stores_back_post_arith_state() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = R[0] + R[1]; call R[2]
    let prog = [
        Inst::iabc(Op::Add, 0, 0, 1, false),
        Inst::iabc(Op::Call, 2, 1, 0, false),
    ];
    let rec = closed_record(p, 0, &prog);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 100;
    state[1] = 7;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // Call is at index 1 → pc 1.
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 1);
    // The Add ran before the Call's side-exit, so the
    // post-Add value lives in reg_state[0].
    assert_eq!(state[0], 107, "post-arith state visible to interp");
    assert_eq!(state[1], 7);
}

#[test]
fn ops_after_first_call_are_silently_dropped() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // [Add, Call, Mul] — Mul never executes; specifically, a
    // Mul reading R[200] (out of bounds) would normally bail
    // the lowerer, but post-truncation ops are skipped so it
    // compiles fine.
    let prog = [
        Inst::iabc(Op::Add, 0, 1, 2, false),
        Inst::iabc(Op::Call, 3, 1, 0, false),
        Inst::iabc(Op::Mul, 0, 200, 200, false), // would-be OOB if validated
    ];
    let rec = closed_record(p, 0, &prog);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec)
        .expect("compile despite post-truncation OOB");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 30;
    state[2] = 12;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        1,
        "side-exit at Call.pc = 1"
    );
    // The Mul never ran — R[0] holds the Add's result, not
    // R[0]*200.
    assert_eq!(state[0], 42);
}

#[test]
fn cmp_immediately_before_call_bails() {
    // The cmp's "took the Jmp" recording requires a Jmp at
    // cmp_pc + 1. If that slot is an Op::Call instead, the
    // recorded direction can't be lowered as the lowerer understands
    // it — bail.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Lt, 0, 1, 0, true),
        Inst::iabc(Op::Call, 2, 1, 0, false),
    ];
    let rec = closed_record(p, 0, &prog);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn call_a_register_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // p.max_stack ≈ 5; A=200 is way past it.
    let rec = closed_record(p, 0, &[Inst::iabc(Op::Call, 200, 1, 0, false)]);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn cmp_then_jmp_then_call_truncation() {
    // [Lt, Jmp, Add, Call] — the cmp + Jmp pair behaves
    // normally (Jmp consumed by cmp); Add runs; Call truncates.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Lt, 0, 1, 0, true),
        Inst::isj(Op::Jmp, -1),
        Inst::iabc(Op::Add, 0, 0, 2, false),
        Inst::iabc(Op::Call, 3, 1, 0, false),
    ];
    let rec = closed_record(p, 0, &prog);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // R[0]=5, R[1]=10 → 5 < 10 holds → cmp matches K=1 → continue.
    // R[0] += R[2] = 5 + 7 = 12.
    // Call at pc 3 side-exits.
    state[0] = 5;
    state[1] = 10;
    state[2] = 7;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        3,
        "side-exit at Call.pc = 3"
    );
    assert_eq!(state[0], 12);

    // Now flip so cmp doesn't match: R[0]=99, R[1]=10 → 99<10
    // false → side-exit at cmp_pc+2 = 2 (NOT the Call's pc).
    state[0] = 99;
    state[1] = 10;
    state[2] = 7;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        2,
        "cmp side-exit takes precedence over Call truncation"
    );
    assert_eq!(state[0], 99, "Add never ran on this path");
}

#[test]
fn forloop_continues_internal_loop_until_count_hits_zero() {
    // Two-block trace shape: a body op (Add) plus the trailing
    // ForLoop. Internal loop runs natively until count == 0,
    // then side-exits at forloop.pc + 1.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Add, 0, 0, 3, false), // R[0] += R[3]
        // ForLoop on R[1..R[1+3]], jumping back to the Add (the head)
        Inst::iabx(Op::ForLoop, 1, 2),
    ];
    let rec = closed_record(p, 0, &prog);
    let opts = CompileOptions {
        internal_loop: true,
        pre53: false,
        aot: false,
        tier: Default::default(),
        tier_up_at: 0,
    };
    let ct = try_compile_trace_with_options(vm.jit.storage.as_mut(), &rec, opts).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // R[0] = accumulator, R[3] = step for the body's add.
    // R[1] = cur loop var (init 1), R[2] = count (5 iters),
    // R[3] = for-loop step (1), R[4] = visible loop var.
    // BUT R[3] is shared between the body's Add and ForLoop's
    // step — both are 1, so the test setup happens to work
    // (a realistic Proto would use disjoint slots).
    state[0] = 0;
    state[1] = 1;
    state[2] = 5; // count
    state[3] = 1; // step / body add operand
    state[4] = 0;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // ForLoop at index 1, pc=1; exit PC = pc+1 = 2.
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        2,
        "ForLoop count-exhaustion side-exits at pc+1"
    );
    // Body order: Add runs BEFORE ForLoop's count check. With
    // count = 5 initially, body iters: count=5 (Add → 1), 4, 3,
    // 2, 1, 0 (ForLoop's check sees 0, side-exits AFTER the
    // Add ran). So Add fires 6 times — R[0] = 6.
    assert_eq!(state[0], 6);
    // R[2] (count) decremented to 0.
    assert_eq!(state[2], 0);
}

#[test]
fn forloop_one_shot_returns_body_pc_on_continue() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // ForLoop only — empty body, single iter dispatch.
    let prog = [Inst::iabc(Op::ForLoop, 0, 0, 0, false)];
    let rec = closed_record(p, 9, &prog);
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile one-shot ForLoop trace");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 10;
    state[1] = 3; // count > 0 → continue
    state[2] = 1; // step
    state[3] = 0;
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // ForLoop continue returns the BODY START pc = (rop.pc + 1) -
    // bx, not head_pc: re-dispatching the same ForLoop op would
    // double-advance the counter. In this synthetic test the
    // closed_record helper assigns sequential pcs starting from
    // 0, so rop.pc=0, bx=0, body_pc=1. For a real
    // loop with non-zero bx, body_pc would land on the loop body
    // start; for this synthetic op chain body_pc just exits past
    // the ForLoop.
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        1,
        "one-shot continue returns body_pc=(rop.pc+1)-bx=1"
    );
    // R[0] = 10 + 1 = 11.
    assert_eq!(state[0], 11);
    assert_eq!(state[1], 2); // count -= 1
    assert_eq!(state[3], 11); // visible loop var = next
}

#[test]
fn forloop_pre53_compiles_the_limit_form() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [Inst::iabc(Op::ForLoop, 0, 0, 0, false)];
    let rec = closed_record(p, 0, &prog);
    let opts = CompileOptions {
        internal_loop: false,
        pre53: true,
        aot: false,
        tier: Default::default(),
        tier_up_at: 0,
    };
    assert!(try_compile_trace_with_options(vm.jit.storage.as_mut(), &rec, opts).is_some());
}

#[test]
fn forloop_a_plus_3_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // max_stack = 5; A=3 → A+3 = 6 ≥ 5 → bail.
    let prog = [Inst::iabc(Op::ForLoop, 3, 0, 0, false)];
    let rec = closed_record(p, 0, &prog);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn op_call_then_unwhitelisted_op_still_compiles() {
    // Even if a non-whitelisted op (Op::Return0) appears after
    // the first Call, the truncation drops it before validation.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Call, 0, 1, 0, false),
        Inst::iabc(Op::Return0, 0, 0, 0, false), // would bail if validated
    ];
    let rec = closed_record(p, 0, &prog);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec)
        .expect("compile despite post-truncation Return0");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // SAFETY: `ct`'s code is owned by `vm`'s JIT storage, `state` has at
    // least the proto's `max_stack` slots, and the trace's integer ops
    // call no helper (it ends at its `Call`)
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
}
