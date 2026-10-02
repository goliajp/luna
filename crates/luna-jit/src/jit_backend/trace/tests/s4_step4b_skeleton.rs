//! frame-mat helper + data structures. These tests pin:
//!   - FrameMaterializeInfo layout is repr(C) (12 bytes amd64)
//!   - `per_exit_inline` is empty for traces with no cmp@d>0 site
//!   - the helper returns -1 when there is no live Lua frame
use super::*;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;
use luna_core::vm::isa::{Inst, Op};

const WIDE_SRC: &[u8] = b"local a,b,c,d = 0,0,0,0; return a+b+c+d";

fn load_proto(vm: &mut Vm, src: &[u8]) -> Gc<Proto> {
    vm.load(src, b"=t").expect("compile").proto
}

#[test]
fn frame_materialize_info_layout_is_stable() {
    // 12-byte layout on every supported target — 4 + 4 + 4. If
    // padding ever sneaks in here the IR's pointer-arithmetic
    // load would read garbage.
    assert_eq!(std::mem::size_of::<FrameMaterializeInfo>(), 12);
    assert_eq!(std::mem::align_of::<FrameMaterializeInfo>(), 4);
}

#[test]
fn compiled_traces_have_empty_per_exit_metas_when_no_inline_cmp() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // A plain depth=0 add — no cmp@d>0 site, so per_exit_inline
    // stays empty.
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
        inline_depth: 0,
        var_count: None,
    });
    rec.closed = true;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("simple add compiles");
    assert!(
        ct.per_exit_inline.is_empty(),
        "no cmp@d>0 site → per_exit_inline empty"
    );
}

#[test]
fn helper_with_no_lua_frame_returns_deopt() {
    // No `enter_jit` guard + no Lua frame at trace head → helper
    // hits the `jit_last_lua_frame()` None branch and returns -1
    // (deopt sentinel). We seed an enter_jit so `current_jit_vm`
    // resolves but leave `vm.frames` empty (test-only Vm starts
    // with no Lua frame).
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    // Need a closure pointer for `current_jit_closure`. Load a
    // tiny chunk and pin its main closure.
    let cl = vm.load(WIDE_SRC, b"=t").expect("compile");
    let metas: [FrameMaterializeInfo; 0] = [];
    let r = {
        let _g = crate::jit_backend::enter_jit(&mut vm, Some(cl));
        unsafe { crate::jit_backend::luna_jit_trace_materialize_frames(0, metas.as_ptr()) }
    };
    assert_eq!(r, -1, "no live Lua frame → helper returns deopt sentinel");
}

/// Helper with a live trace-head frame pushes N inlined
/// frames with `base = head.base + meta.base_offset`, `pc` from
/// the meta, `func_slot = base - 1`, and `nresults` from the
/// meta. `cl.proto` is the same closure pinned by enter_jit.
#[test]
fn helper_pushes_one_inlined_frame_with_correct_metadata() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    // Run a tiny program so vm.frames ends with a live Lua frame
    // — actually, after `eval` returns the frames are popped.
    // Easier route: drive a recording-like fixture. Use the
    // existing `enter_jit_dispatch` machinery by hand isn't
    // worth it for a unit; instead seed a frame directly.
    let cl = vm.load(WIDE_SRC, b"=t").expect("compile");
    // Push a synthetic head frame so `jit_last_lua_frame` returns Some.
    vm.jit_ensure_stack(64);
    vm.jit_push_inlined_frame(cl, /*base*/ 1, /*pc*/ 7, /*nresults*/ 1);
    let frames_before = {
        // borrow vm just to count — use the accessor via guard
        // scope below to compose without lifetime pain.
        let _g = crate::jit_backend::enter_jit(&mut vm, Some(cl));
        // can't call vm methods inside _g scope (vm moved into
        // accessor); drop guard first.
        drop(_g);
        // recompute after guard drop
        // (intentional: count via the Vm public surface — there
        // isn't a frame_count() public accessor, so we use the
        // fact that the helper returns 0 for success and the
        // post-push assertion below covers count via the
        // last-frame's base.)
        0
    };
    let _ = frames_before;
    let metas = [FrameMaterializeInfo {
        base_offset: 5,
        pc: 11,
        nresults: 1,
    }];
    let r = {
        let _g = crate::jit_backend::enter_jit(&mut vm, Some(cl));
        unsafe { crate::jit_backend::luna_jit_trace_materialize_frames(1, metas.as_ptr()) }
    };
    assert_eq!(r, 0, "successful push returns 0");
    // The just-pushed frame has base = head.base + 5 = 1 + 5 = 6,
    // pc = 11, nresults = 1, func_slot = base - 1 = 5.
    let pushed = vm.jit_last_lua_frame().expect("frame was pushed");
    assert_eq!(pushed.base, 6);
    assert_eq!(pushed.pc, 11);
    assert_eq!(pushed.func_slot, 5);
    assert_eq!(pushed.nresults, 1);
    assert_eq!(pushed.n_varargs, 0);
}

/// A single self-recursive Call followed by a cmp@d=1
/// produces ONE per_exit_metas entry — the cmp's chain has one
/// frame (the inlined callee) with base_offset matching
/// op_offsets[2] and pc overridden to the cmp's side-exit PC
/// (`cmp.pc + 2`, the TookJmp direction).
#[test]
fn per_exit_metas_populated_for_cmp_at_depth_one() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    // Non-vararg inner proto needed — the vararg bail
    // refuses the self-rec inline path on a vararg head.
    let cl = vm
        .load(b"local function f(a,b) return a+b end return f", b"=t")
        .expect("compile");
    let p = cl.proto.protos[0];
    assert!(!p.is_vararg);
    // R[0] holds the function itself: an inlined call needs its
    // target to be the entry closure
    let mut tags = vec![luna_core::runtime::value::raw::INT; p.max_stack as usize];
    tags[0] = luna_core::runtime::value::raw::CLOSURE;
    let mut rec = TraceRecord::start(p, 0, tags, false);
    // depth 0: an Add, then a self-recursive Call.
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
        inst: Inst::iabc(Op::Call, 0, 1, 2, false), // A=0, C=2 → nresults 1
        inline_depth: 0,
        var_count: None,
    });
    // depth 1: a cmp + the trailing Jmp the cmp consumes
    // (TookJmp direction). Side-exit PC = cmp.pc + 2 = 2.
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
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compiles via inline-cmp");
    assert_eq!(
        ct.per_exit_inline.len(),
        1,
        "one cmp@d>0 site → one per-exit-inline entry"
    );
    let info = &ct.per_exit_inline[0];
    // Cmp at depth=1 pc=0; TookJmp → side-exit pc = cmp.pc + 2 = 2.
    assert_eq!(info.cont_pc, 2);
    assert_eq!(info.chain.len(), 1, "one inlined frame in chain");
    let m = info.chain[0];
    // Call A=0 → callee base_offset = A + 1 = 1.
    assert_eq!(m.base_offset, 1);
    // Innermost frame's pc was overridden from caller-resume
    // (= Call.pc + 1 = 2) to the side-exit pc (also 2 here, but
    // could differ for SkippedJmp or non-trivial layouts).
    assert_eq!(m.pc, 2);
    assert_eq!(m.nresults, 1);
}

/// Op::Call with C != 2 (i.e. nresults != 1) bails
/// the whole trace — the Op::Return1 copy-back assumes one
/// value, and the helper passes through whatever `nresults` the
/// meta says without validating.
#[test]
fn self_recursive_call_with_multiple_returns_bails() {
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
        inst: Inst::iabc(Op::Call, 0, 1, 3, false), // C=3 → nresults=2
        inline_depth: 0,
        var_count: None,
    });
    rec.push(RecordedOp {
        proto: p,
        pc: 0,
        inst: Inst::iabc(Op::Add, 0, 1, 2, false),
        inline_depth: 1,
        var_count: None,
    });
    rec.closed = true;
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn helper_pushes_multiple_frames_in_order() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let cl = vm.load(WIDE_SRC, b"=t").expect("compile");
    vm.jit_ensure_stack(64);
    vm.jit_push_inlined_frame(cl, 1, 0, 1);
    let metas = [
        FrameMaterializeInfo {
            base_offset: 3,
            pc: 7,
            nresults: 1,
        },
        FrameMaterializeInfo {
            base_offset: 8,
            pc: 7,
            nresults: 1,
        },
        FrameMaterializeInfo {
            base_offset: 13,
            pc: 9,
            nresults: 1,
        },
    ];
    let r = {
        let _g = crate::jit_backend::enter_jit(&mut vm, Some(cl));
        unsafe { crate::jit_backend::luna_jit_trace_materialize_frames(3, metas.as_ptr()) }
    };
    assert_eq!(r, 0);
    // Innermost frame should match metas[2].
    let inner = vm.jit_last_lua_frame().expect("inner frame");
    assert_eq!(inner.base, 1 + 13);
    assert_eq!(inner.pc, 9);
}
