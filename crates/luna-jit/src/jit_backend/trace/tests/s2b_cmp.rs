use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const WIDE_SRC: &[u8] = b"local a,b,c,d = 0,0,0,0; return a+b+c+d";

fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    let cl = vm.load(src, b"=t").expect("compile");
    cl.proto
}

/// Build a 2-op trace `[cmp, jmp_back]` of length 2. `cmp_pc`
/// is what the cmp's `RecordedOp.pc` will be; the trailing Jmp
/// gets `cmp_pc + 1` so it satisfies the "Jmp at cmp.pc + 1"
/// rule. The trace's `head_pc` is supplied separately — it
/// controls the clean-close return value and is independent of
/// the cmp's PC.
fn cmp_jmp_record(proto: Gc<Proto>, head_pc: u32, cmp_pc: u32, cmp: Inst) -> TraceRecord {
    let mut rec = TraceRecord::start(
        proto,
        head_pc,
        vec![luna_core::runtime::value::raw::INT; proto.max_stack as usize],
        false,
    );
    rec.push(RecordedOp {
        proto,
        pc: cmp_pc,
        inst: cmp,
        inline_depth: 0,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto,
        pc: cmp_pc + 1,
        inst: Inst::isj(Op::Jmp, -1),
        inline_depth: 0,
        var_count: None,
    });
    rec.closed = true;
    rec
}

#[test]
fn lt_k1_returns_head_pc_when_cmp_matches() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // if (R[1] < R[2]) ~= 1 then pc++   — recorded "1 < 2", matched K=1.
    let lt = Inst::iabc(Op::Lt, 1, 2, 0, true);
    let rec = cmp_jmp_record(p, 5, 10, lt);
    let ct =
        try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("Lt + trailing Jmp must compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 3; // 3 < 7 holds → matches K=true → continue
    state[2] = 7;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        5,
        "clean close returns head_pc"
    );
}

#[test]
fn lt_k1_side_exits_when_cmp_inverts() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let lt = Inst::iabc(Op::Lt, 1, 2, 0, true); // K=1
    let rec = cmp_jmp_record(p, 5, 10, lt);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 9; // 9 < 7 false → mismatch with K=1 → side-exit
    state[2] = 7;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // Lua's `pc++` on cmp mismatch lands at cmp_pc + 2 = 12.
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        12,
        "side-exit returns failing PC = cmp_pc + 2"
    );
    // Reg state is still written back so interp resumes
    // with consistent values.
    assert_eq!(state[1], 9);
    assert_eq!(state[2], 7);
}

#[test]
fn lt_k0_inverts_continue_condition() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // if (R[1] < R[2]) ~= 0 then pc++  — recorded direction:
    // cmp result was false (9 < 7), matched K=0 → took Jmp.
    let lt = Inst::iabc(Op::Lt, 1, 2, 0, false);
    let rec = cmp_jmp_record(p, 3, 8, lt);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 9;
    state[2] = 7;
    // Cmp result `9 < 7` is false; K=0; false == K=0 → continue.
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 3);

    // Flip inputs so cmp result `3 < 7` is true; K=0; true != K → side-exit.
    state[1] = 3;
    state[2] = 7;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    assert_eq!(
        crate::jit_backend::trace::exit_pc(r),
        10,
        "cmp_pc=8 + 2 = 10"
    );
}

#[test]
fn le_emits_signed_less_equal() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // if (R[1] <= R[2]) ~= 1 then pc++
    let le = Inst::iabc(Op::Le, 1, 2, 0, true);
    let rec = cmp_jmp_record(p, 0, 0, le);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // Equal: 5 <= 5 holds → continue.
    state[1] = 5;
    state[2] = 5;
    assert_eq!(
        crate::jit_backend::trace::exit_pc(unsafe { (ct.entry)(state.as_mut_ptr()) }),
        0
    );

    // 5 <= 4 false → side-exit.
    state[1] = 5;
    state[2] = 4;
    assert_eq!(
        crate::jit_backend::trace::exit_pc(unsafe { (ct.entry)(state.as_mut_ptr()) }),
        2
    );
}

#[test]
fn eq_emits_int_equality() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // if (R[0] == R[1]) ~= 1 then pc++
    let eq = Inst::iabc(Op::Eq, 0, 1, 0, true);
    let rec = cmp_jmp_record(p, 7, 4, eq);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 42;
    state[1] = 42;
    assert_eq!(
        crate::jit_backend::trace::exit_pc(unsafe { (ct.entry)(state.as_mut_ptr()) }),
        7
    );

    state[0] = 42;
    state[1] = 41;
    assert_eq!(
        crate::jit_backend::trace::exit_pc(unsafe { (ct.entry)(state.as_mut_ptr()) }),
        6
    ); // cmp_pc(4) + 2
}

#[test]
fn arith_then_cmp_side_exit_stores_back_post_arith_value() {
    // Trace records: R[0] = R[0] - R[1]; Lt R[0] R[2] K=1; Jmp -3.
    // The side-exit must write the *post-arith* R[0] to reg_state
    // so interp resumes with the value the trace just computed.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Sub, 0, 0, 1, false),
        Inst::iabc(Op::Lt, 0, 2, 0, true), // R[0] < R[2]
        Inst::isj(Op::Jmp, -3),
    ];
    let mut rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    for (i, inst) in prog.iter().copied().enumerate() {
        rec.push(RecordedOp {
            proto: p,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
    }
    rec.closed = true;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    // R[0] = 10 - 7 = 3; 3 < 5 → continue → return head_pc.
    state[0] = 10;
    state[1] = 7;
    state[2] = 5;
    assert_eq!(
        crate::jit_backend::trace::exit_pc(unsafe { (ct.entry)(state.as_mut_ptr()) }),
        0
    );
    assert_eq!(state[0], 3);

    // R[0] = 10 - 1 = 9; 9 < 5 false → side-exit.
    state[0] = 10;
    state[1] = 1;
    state[2] = 5;
    let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
    // cmp at index 1, pc=1; failing PC = pc+2 = 3.
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 3);
    assert_eq!(state[0], 9, "post-arith value must be in reg_state");
    assert_eq!(state[1], 1);
    assert_eq!(state[2], 5);
}

#[test]
fn cmp_at_trailing_position_bails() {
    // No Jmp follows — the lowerer doesn't capture the
    // "cmp didn't match K, Jmp skipped" direction.
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
        inst: Inst::iabc(Op::Lt, 0, 1, 0, true),
        inline_depth: 0,
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn cmp_followed_by_non_jmp_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Lt, 0, 1, 0, true),
        Inst::iabc(Op::Add, 2, 0, 1, false), // not a Jmp
    ];
    let mut rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    for (i, inst) in prog.iter().copied().enumerate() {
        rec.push(RecordedOp {
            proto: p,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
    }
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn cmp_jmp_with_wrong_pc_offset_bails() {
    // Jmp must be at cmp.pc + 1. A Jmp at cmp.pc + 5 is treated
    // as an orphan and bails.
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
        inst: Inst::iabc(Op::Lt, 0, 1, 0, true),
        inline_depth: 0,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto: p,
        pc: 5, // not 1
        inst: Inst::isj(Op::Jmp, -1),
        inline_depth: 0,
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn orphan_jmp_mid_trace_bails() {
    // A Jmp that's not consumed by a cmp and not at the last
    // position must bail (the lowerer doesn't know what to do with
    // a free-floating unconditional jump).
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::isj(Op::Jmp, -1), // orphan, position 0 of a 2-op record
        Inst::iabc(Op::Add, 0, 1, 2, false),
    ];
    let mut rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    for (i, inst) in prog.iter().copied().enumerate() {
        rec.push(RecordedOp {
            proto: p,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
    }
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn loop_pattern_alternates_continue_and_exit() {
    // Simulate a count-down loop:
    //   loop: R[0] = R[0] - R[1];  Lt R[0] R[2] K=0 (R[0] >= R[2] → continue);  Jmp -3
    // Continue while R[0] >= R[2]; side-exit when R[0] < R[2].
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let prog = [
        Inst::iabc(Op::Sub, 0, 0, 1, false),
        Inst::iabc(Op::Lt, 0, 2, 0, false), // K=0
        Inst::isj(Op::Jmp, -3),
    ];
    let mut rec = TraceRecord::start(
        p,
        0,
        vec![luna_core::runtime::value::raw::INT; p.max_stack as usize],
        false,
    );
    for (i, inst) in prog.iter().copied().enumerate() {
        rec.push(RecordedOp {
            proto: p,
            pc: i as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        });
    }
    rec.closed = true;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = 20;
    state[1] = 3; // decrement
    state[2] = 5; // floor

    // Iterate the trace as a real dispatcher would. Expect:
    // 20 → 17, 17 → 14, ..., 8 → 5 (continues; 5 >= 5 still
    // matches K=0 since 5 < 5 is false), 5 → 2 (side-exits
    // because 2 < 5 = true, mismatch).
    let mut iters = 0;
    loop {
        let r = unsafe { (ct.entry)(state.as_mut_ptr()) };
        iters += 1;
        if r != 0 {
            assert_eq!(
                crate::jit_backend::trace::exit_pc(r),
                3,
                "side-exit PC = cmp_pc(1) + 2"
            );
            break;
        }
        assert!(iters < 100, "loop should terminate");
    }
    assert_eq!(state[0], 2);
    // Iterations: 20→17→14→11→8→5→2 = 6 successful subtracts
    // (5 of them with R[0] >= 5 → continue, then 2 < 5 →
    // side-exit). So iters == 6.
    assert_eq!(iters, 6);
}
