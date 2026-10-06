//! `compute_op_offsets` correctness tests.
//!
//! The helper is a pure function over `TraceRecord.ops`'s
//! `inline_depth` field. These tests build synthetic records
//! with hand-set depths + `Op::Call` ops to verify the offset
//! stack matches the expected per-frame register window base.
//! Step 3b will start consuming the helper's output in the
//! body emit pass.

use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    vm.load(src, b"=t").expect("compile").proto
}

fn make_record(proto: Gc<Proto>, items: Vec<(Inst, u8)>) -> TraceRecord {
    // The registers these traces compute on hold integers.
    let tags = vec![luna_core::runtime::value::raw::INT; proto.max_stack as usize];
    let mut rec = TraceRecord::start(proto, 0, tags, true);
    for (i, (inst, depth)) in items.into_iter().enumerate() {
        let pushed = rec.push(RecordedOp {
            proto,
            pc: i as u32,
            inst,
            inline_depth: depth,
            var_count: None,
        });
        assert!(pushed, "rec.push should not overflow");
    }
    rec
}

/// Pure depth-0 trace → all offsets stay at 0.
#[test]
fn depth_zero_only_yields_zero_offsets() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, b"return 0");
    let rec = make_record(
        p,
        vec![
            (Inst::iabc(Op::LoadI, 0, 0, 128, false), 0),
            (Inst::iabc(Op::Add, 1, 0, 0, false), 0),
            (Inst::iabc(Op::Return1, 1, 0, 0, false), 0),
        ],
    );
    let (offsets, enclosing) = compute_op_offsets(&rec);
    assert_eq!(offsets, vec![0u32, 0, 0]);
    assert_eq!(enclosing, vec![None, None, None]);
}

/// Depth 0 → 1 → 0 via Op::Call(A=3) then Op::Return.
/// Callee's offset = 0 + 3 + 1 = 4; back to depth 0 → 0.
#[test]
fn single_call_bumps_then_drops_offset() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, b"return 0");
    let rec = make_record(
        p,
        vec![
            (Inst::iabc(Op::Move, 0, 0, 0, false), 0),
            // no argument: the callee (this vararg chunk) has no extra
            // arguments below its registers
            (Inst::iabc(Op::Call, 3, 1, 2, false), 0), // A=3
            (Inst::iabc(Op::LoadI, 0, 0, 128, false), 1), // callee R[0]
            (Inst::iabc(Op::Return1, 0, 0, 0, false), 1),
            (Inst::iabc(Op::Move, 4, 3, 0, false), 0), // back to depth 0
        ],
    );
    let (offsets, enclosing) = compute_op_offsets(&rec);
    assert_eq!(offsets, vec![0u32, 0, 4, 4, 0]);
    // depth 0,0,1,1,0 → enclosing 0,0,Some(3),Some(3),0 (A=3 from Op::Call)
    assert_eq!(enclosing, vec![None, None, Some(3), Some(3), None]);
}

/// Nested calls: depth 0 → 1 → 2. Each Op::Call A is captured.
/// Caller's A=2 → callee_1 offset = 0+2+1 = 3.
/// Callee_1's A=4 → callee_2 offset = 3+4+1 = 8.
#[test]
fn nested_calls_accumulate_offsets() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, b"return 0");
    let rec = make_record(
        p,
        vec![
            (Inst::iabc(Op::Call, 2, 1, 0, false), 0),
            (Inst::iabc(Op::Call, 4, 1, 0, false), 1),
            (Inst::iabc(Op::LoadI, 0, 0, 128, false), 2),
            (Inst::iabc(Op::Return0, 0, 0, 0, false), 2),
            (Inst::iabc(Op::Return0, 0, 0, 0, false), 1),
            (Inst::iabc(Op::Move, 0, 0, 0, false), 0),
        ],
    );
    let (offsets, enclosing) = compute_op_offsets(&rec);
    // Expected:
    //   ops[0] = Call A=2,           depth=0 → offset 0
    //   ops[1] = Call A=4 (callee_1) depth=1 → offset = 0+2+1 = 3
    //   ops[2] = LoadI (callee_2)    depth=2 → offset = 3+4+1 = 8
    //   ops[3] = Return0             depth=2 → offset 8
    //   ops[4] = Return0             depth=1 → offset 3
    //   ops[5] = Move                depth=0 → offset 0
    assert_eq!(offsets, vec![0u32, 3, 8, 8, 3, 0]);
    assert_eq!(
        enclosing,
        vec![None, Some(2), Some(4), Some(4), Some(2), None]
    );
}
