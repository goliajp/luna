//! The side-trace gate declines a side trace compiled to loop on itself
//! that closes over a loop edge while it writes shared state: running its
//! body again on the next pass would repeat the write. A `Jmp` keeps its
//! offset in the `sJ` field; read as `sBx`, a backward jump of fewer than
//! 256 instructions looked like no jump at all. Side traces are compiled
//! one-shot, and then a loop edge they recorded is straight-line code: such
//! a side trace compiles and runs (`side_trace_through_an_inner_loop`).

use luna_jit::jit::trace::{
    CompileOptions, RecordedOp, TraceRecord, last_compile_checkpoint, try_compile_trace,
    try_compile_trace_with_options,
};
use luna_jit::runtime::Gc;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::value::raw;
use luna_jit::version::LuaVersion;
use luna_jit::vm::isa::{Inst, Op};

const IMPURE_BACK_EDGE: &str = "bail:side-trace-back-edge-with-impure";

/// Options for a trace compiled to loop on itself.
fn looping() -> CompileOptions {
    CompileOptions {
        internal_loop: true,
        ..CompileOptions::default()
    }
}

fn proto_of(vm: &mut luna_jit::vm::Vm, src: &str) -> Gc<Proto> {
    vm.load(src.as_bytes(), b"=t").expect("compile").proto
}

fn pc_of(proto: Gc<Proto>, op: Op) -> u32 {
    proto
        .code
        .iter()
        .position(|i| i.op() == op)
        .expect("op in chunk") as u32
}

/// A side trace of `proto` (its parent: the trace at pc 0, exit 0) made of
/// the instructions at `pcs`, the last one replaced by `last` if given.
fn side_record(proto: Gc<Proto>, pcs: &[u32], last: Option<Inst>) -> TraceRecord {
    let mut rec = TraceRecord::start_side_trace(
        proto,
        pcs[0],
        vec![raw::INT; proto.max_stack as usize],
        proto,
        0,
        0,
    );
    for (k, &pc) in pcs.iter().enumerate() {
        let inst = match last {
            Some(i) if k + 1 == pcs.len() => i,
            _ => proto.code[pc as usize],
        };
        assert!(rec.push(RecordedOp {
            proto,
            pc,
            inst,
            inline_depth: 0,
            var_count: None,
        }));
    }
    rec.closed = true;
    rec
}

const WHILE_WRITE: &str = "local t, j, n = {}, 0, 3
    while j < n do
      t[j] = j
      j = j + 1
    end
    return t";

/// The body of the `while` from its table write to its backward jump.
fn while_body(p: Gc<Proto>) -> Vec<u32> {
    let set = pc_of(p, Op::SetTable);
    let jmp = (set..p.code.len() as u32)
        .find(|&pc| p.code[pc as usize].op() == Op::Jmp)
        .expect("loop jump");
    assert!(p.code[jmp as usize].sj() < 0 && p.code[jmp as usize].sj() > -256);
    (set..=jmp).collect()
}

#[test]
fn short_backward_jump_with_a_table_write_is_declined() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    let rec = side_record(p, &while_body(p), None);
    assert!(try_compile_trace_with_options(vm.jit.storage.as_mut(), &rec, looping()).is_none());
    assert_eq!(last_compile_checkpoint(), IMPURE_BACK_EDGE);
}

#[test]
fn long_backward_jump_with_a_table_write_is_declined() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    let rec = side_record(p, &while_body(p), Some(Inst::isj(Op::Jmp, -300)));
    assert!(try_compile_trace_with_options(vm.jit.storage.as_mut(), &rec, looping()).is_none());
    assert_eq!(last_compile_checkpoint(), IMPURE_BACK_EDGE);
}

#[test]
fn forward_jump_with_a_table_write_passes_the_gate() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    for sj in [0, 1, 200] {
        let rec = side_record(p, &while_body(p), Some(Inst::isj(Op::Jmp, sj)));
        let _ = try_compile_trace_with_options(vm.jit.storage.as_mut(), &rec, looping());
        assert_ne!(last_compile_checkpoint(), IMPURE_BACK_EDGE, "sJ {sj}");
    }
}

#[test]
fn one_shot_side_trace_takes_its_recorded_loop_edge() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    let rec = side_record(p, &while_body(p), None);
    let _ = try_compile_trace(vm.jit.storage.as_mut(), &rec);
    assert_ne!(last_compile_checkpoint(), IMPURE_BACK_EDGE);
}

/// The trace compiler, except that it refuses a loop trace headed at a
/// comparison: here the inner `while`'s own trace, which would otherwise
/// take over the inner loop before a side trace could run through it.
struct NoInnerLoopTrace;

impl luna_jit::jit::TraceCompiler for NoInnerLoopTrace {
    fn try_compile_trace(
        &self,
        s: &mut dyn luna_jit::jit::JitStorage,
        r: &TraceRecord,
        o: CompileOptions,
    ) -> Option<luna_jit::jit::trace::CompiledTrace> {
        self.try_compile_trace_for(s, r, o, LuaVersion::Lua54)
    }

    fn try_compile_trace_for(
        &self,
        s: &mut dyn luna_jit::jit::JitStorage,
        r: &TraceRecord,
        o: CompileOptions,
        v: LuaVersion,
    ) -> Option<luna_jit::jit::trace::CompiledTrace> {
        let head = r.head_proto.code[r.head_pc as usize].op();
        if r.side_trace_parent.is_none() && matches!(head, Op::Lt | Op::Le | Op::LtI | Op::LeI) {
            return None;
        }
        luna_jit::jit::CraneliftBackend.try_compile_trace_for(s, r, o, v)
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        luna_jit::jit::CraneliftBackend.last_compile_checkpoint()
    }

    fn tier_up(
        &self,
        s: &mut dyn luna_jit::jit::JitStorage,
        ct: &luna_jit::jit::trace::CompiledTrace,
    ) -> Option<luna_jit::jit::trace::TraceFn> {
        luna_jit::jit::CraneliftBackend.tier_up(s, ct)
    }
}

/// A side trace from the outer loop's trace that goes round the inner
/// `while` (its backward jump, the loop test, a table write) back to where
/// it started: compiled before only when the recording did not end on the
/// loop test (it does, right before the head) and its loop edge was not
/// taken for a body jump the trace could not follow.
#[test]
fn side_trace_through_an_inner_loop() {
    let src = "local t, s = {}, 0
        for i = 1, 400 do
          local j, n = 0, i % 7
          while j < n do
            t[j] = (t[j] or 0) + i
            j = j + 1
          end
          s = s + j
        end
        local sum = 0
        for k = 0, 6 do sum = sum + (t[k] or 0) end
        return s, sum";
    for tier in [
        luna_jit::jit::trace::TraceTier::Baseline,
        luna_jit::jit::trace::TraceTier::Optimizing,
    ] {
        let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
        vm.set_jit_enabled(false);
        vm.install_jit_backend(luna_jit::jit::CraneliftBackend, NoInnerLoopTrace);
        vm.jit.trace_hot_threshold = 8;
        vm.set_trace_tier(tier);
        let r = vm.eval(src).expect("run");
        assert!(
            matches!(
                r[..],
                [
                    luna_jit::runtime::Value::Int(1198),
                    luna_jit::runtime::Value::Int(240199)
                ]
            ),
            "{tier:?}: {r:?}"
        );
        assert!(
            vm.trace_side_trace_run_count() > 0,
            "{tier:?}: no side trace ran"
        );
    }
}
