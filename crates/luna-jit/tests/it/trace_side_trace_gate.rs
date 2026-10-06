//! The side-trace gate declines a side trace that closes over a loop edge
//! while it writes shared state: running its body again on the next pass
//! would repeat the write. A `Jmp` keeps its offset in the `sJ` field; read
//! as `sBx`, a backward jump of fewer than 256 instructions looked like no
//! jump at all.

use luna_jit::jit::trace::{RecordedOp, TraceRecord, last_compile_checkpoint, try_compile_trace};
use luna_jit::runtime::Gc;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::value::raw;
use luna_jit::version::LuaVersion;
use luna_jit::vm::isa::{Inst, Op};

const IMPURE_BACK_EDGE: &str = "bail:side-trace-back-edge-with-impure";

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
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
    assert_eq!(last_compile_checkpoint(), IMPURE_BACK_EDGE);
}

#[test]
fn long_backward_jump_with_a_table_write_is_declined() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    let rec = side_record(p, &while_body(p), Some(Inst::isj(Op::Jmp, -300)));
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
    assert_eq!(last_compile_checkpoint(), IMPURE_BACK_EDGE);
}

#[test]
fn forward_jump_with_a_table_write_passes_the_gate() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(&mut vm, WHILE_WRITE);
    for sj in [0, 1, 200] {
        let rec = side_record(p, &while_body(p), Some(Inst::isj(Op::Jmp, sj)));
        let _ = try_compile_trace(vm.jit.storage.as_mut(), &rec);
        assert_ne!(last_compile_checkpoint(), IMPURE_BACK_EDGE, "sJ {sj}");
    }
}
