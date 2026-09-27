//! A self-link close takes the whole recording as the trace body. The
//! body emit has no lowering for a loop edge or a call it does not
//! inline in the middle of a trace, so a recording holding one (an
//! embedder turned self-link on and ran a crafted chunk) is declined
//! rather than compiled or tripping an internal assertion.

use luna_jit::jit::trace::{
    RecordedOp, SelfRecKind, TraceRecord, last_compile_checkpoint, try_compile_trace,
};
use luna_jit::runtime::Gc;
use luna_jit::runtime::function::Proto;
use luna_jit::runtime::value::raw;
use luna_jit::version::LuaVersion;
use luna_jit::vm::isa::{Inst, Op};

fn proto_of(vm: &mut luna_jit::vm::Vm, src: &str) -> Gc<Proto> {
    vm.load(src.as_bytes(), b"=t").expect("compile").proto
}

fn self_link_record(proto: Gc<Proto>, ops: &[(u32, Inst)]) -> TraceRecord {
    let mut rec = TraceRecord::start(proto, 0, vec![raw::INT; proto.max_stack as usize], true);
    for &(pc, inst) in ops {
        assert!(rec.push(RecordedOp {
            proto,
            pc,
            inst,
            inline_depth: 0,
            var_count: None,
        }));
    }
    rec.self_link_kind = Some(SelfRecKind::UpRec);
    rec.closed = true;
    rec
}

fn pc_of(proto: Gc<Proto>, op: Op) -> u32 {
    proto
        .code
        .iter()
        .position(|i| i.op() == op)
        .expect("op in chunk") as u32
}

#[test]
fn loop_edge_inside_a_self_link_body_is_declined() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(
        &mut vm,
        "local s = 0 for i = 1, 3 do s = s + i end return s",
    );
    let fl = pc_of(p, Op::ForLoop);
    let add = pc_of(p, Op::Add);
    let ops = [
        (add, p.code[add as usize]),
        (fl, p.code[fl as usize]),
        (add, p.code[add as usize]),
    ];
    let rec = self_link_record(p, &ops);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
    assert_eq!(last_compile_checkpoint(), "bail:self-link-body-terminator");
}

#[test]
fn call_not_inlined_inside_a_self_link_body_is_declined() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua54);
    let p = proto_of(
        &mut vm,
        "local f, x, y = ... x = x + y f(x) x = x + y return x",
    );
    let call = pc_of(p, Op::Call);
    let add = pc_of(p, Op::Add);
    let ops = [
        (add, p.code[add as usize]),
        (call, p.code[call as usize]),
        (add, p.code[add as usize]),
    ];
    let rec = self_link_record(p, &ops);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
    assert_eq!(last_compile_checkpoint(), "bail:self-link-body-terminator");
}
