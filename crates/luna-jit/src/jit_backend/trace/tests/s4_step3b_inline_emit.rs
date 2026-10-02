//! body emit consumes the offset/enclosing/window
//! triple from `compute_op_offsets`. These tests craft synthetic
//! `TraceRecord`s with depth>0 ops to verify the inline emit
//! paths in isolation.
use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const WIDE_SRC: &[u8] = b"local a,b,c,d = 0,0,0,0; return a+b+c+d";

fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    vm.load(src, b"=t").expect("compile").proto
}

/// A cmp@d>0 emits a real side-exit via the frame-mat helper
/// rather than closing via InlineAbort.
/// The trace is dispatchable (subject to other gates). See
/// `s4_step4b_skeleton::per_exit_metas_populated_for_cmp_at_depth_one`
/// for the positive coverage; this slot is kept as a regression
/// guard against accidentally re-introducing the InlineAbort
/// path for cmp@d>0.
#[test]
fn cmp_at_depth_one_no_longer_aborts_via_inline_abort() {
    // Trace with a non-vararg head proto containing one
    // self-rec Call followed by a cmp+Jmp at depth=1.
    // end_idx_opt finds NO InlineAbort
    // terminator at the cmp — it falls through to normal emit.
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    // Find a non-vararg inner proto: `local function f(a,b)
    // return a+b end` keeps the inner Proto non-vararg.
    let cl = vm
        .load(b"local function f(a,b) return a+b end return f", b"=t")
        .expect("compile");
    let p = cl.proto.protos[0];
    assert!(!p.is_vararg, "fixture must be non-vararg");
    // R[0] holds the function itself: an inlined call needs its
    // target to be the entry closure
    let mut tags = vec![luna_core::runtime::value::raw::INT; p.max_stack as usize];
    tags[0] = luna_core::runtime::value::raw::CLOSURE;
    let mut rec = TraceRecord::start(p, 0, tags, false);
    rec.push(RecordedOp {
        proto: p,
        pc: 0,
        inst: Inst::iabc(Op::Add, 2, 1, 2, false),
        inline_depth: 0,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto: p,
        pc: 1,
        inst: Inst::iabc(Op::Call, 0, 1, 2, false),
        inline_depth: 0,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto: p,
        pc: 0,
        inst: Inst::iabc(Op::Lt, 0, 1, 0, true),
        inline_depth: 1,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto: p,
        pc: 1,
        inst: Inst::isj(Op::Jmp, 0),
        inline_depth: 1,
        var_count: None,
    });
    rec.closed = true;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec)
        .expect("cmp@d>0 now compiles via inline-cmp emit");
    // One cmp@d>0 site → one per-exit-metas entry.
    assert_eq!(ct.per_exit_inline.len(), 1);
    // Window covers caller + inlined frame's slots.
    assert!(
        ct.window_size as usize >= p.max_stack as usize,
        "window_size ({}) must be ≥ max_stack ({})",
        ct.window_size,
        p.max_stack,
    );
}

/// First op at depth>0 violates the recorder invariant and must
/// bail cleanly — `compute_op_offsets` would otherwise underflow
/// the depth-bump arithmetic.
#[test]
fn first_op_at_depth_gt_zero_bails() {
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
        inline_depth: 1, // invariant violation
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

/// First op on a different proto than the trace head also bails
/// (cross-proto on the head op is an invalid recorder state).
#[test]
fn first_op_on_cross_proto_bails() {
    let mut vm1 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let mut vm2 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p1 = load_proto(&mut vm1, WIDE_SRC);
    let p2 = load_proto(&mut vm2, WIDE_SRC);
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
