use object::{Object, ObjectSection};

use luna_core::jit::send_compat::TArc;
use luna_core::jit::trace_types::{CompileOptions, RecordedOp, TraceRecord};
use luna_core::runtime::function::Proto;
use luna_core::runtime::value::raw;
use luna_core::runtime::{Gc, Value};
use luna_core::version::LuaVersion;
use luna_core::vm::isa::{Inst, Op};

use super::super::harvest::Installable;
use super::super::target::TargetSpec;
use super::{build_object_module, emit_meta_sections, lower_and_encode_meta};

fn load_proto(vm: &mut luna_core::vm::Vm) -> Gc<Proto> {
    vm.eval("function add5(a, b, c, d, e) return ((a + b) * c) - d + e end")
        .expect("eval");
    let key = vm.intern_str("add5");
    let g = vm.globals();
    // SAFETY: `g` is the live globals table and nothing else borrows it here
    let v = unsafe { (*g.as_ptr()).get(Value::Str(key)) };
    let Value::Closure(cl) = v else {
        panic!("expected closure for `add5`, got {v:?}");
    };
    // SAFETY: `cl` is a live closure read just above
    unsafe { (*cl.as_ptr()).proto }
}

fn arith_record(proto: Gc<Proto>, closed: bool) -> TraceRecord {
    let ops = [
        Inst::iabc(Op::Add, 0, 1, 2, false),
        Inst::iabc(Op::Mul, 0, 0, 3, false),
        Inst::iabc(Op::Sub, 0, 0, 4, false),
    ];
    let tags = vec![raw::INT; proto.max_stack as usize];
    let mut rec = TraceRecord::start(proto, 0, tags, false);
    for (pc, inst) in ops.into_iter().enumerate() {
        assert!(rec.push(RecordedOp {
            proto,
            pc: pc as u32,
            inst,
            inline_depth: 0,
            var_count: None,
        }));
    }
    rec.closed = closed;
    rec
}

fn meta_section_bytes(obj: &[u8]) -> Vec<u8> {
    let file = object::File::parse(obj).expect("parse emitted object");
    let section = file
        .sections()
        .find(|s| {
            s.name()
                .is_ok_and(|n| n == "luna_trace_meta" || n == ".lt_meta")
        })
        .expect("meta section in emitted object");
    section.data().expect("meta section data").to_vec()
}

// a trace that fails AOT lowering must not shift the hash / head pc of
// the meta entries emitted for the traces after it
#[test]
fn meta_entry_keeps_its_own_trace_after_an_earlier_lowering_bail() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    let proto = load_proto(&mut vm);
    let target = TargetSpec::host();

    let good = arith_record(proto, true);
    let mut scratch = build_object_module(&target).expect("scratch module");
    let (_, ct) = luna_jit::jit_backend::trace::lower_trace_into(
        &mut scratch,
        &good,
        CompileOptions::default(),
    )
    .expect("pure-arith record lowers");
    let ct = TArc::new(ct);

    // an unclosed record bails in the lowerer, standing in for a trace
    // the warmup JIT compiled but `aot: true` codegen rejects
    let bailing = arith_record(proto, false);
    let bail_hash = [0xaa; 16];
    let bail_head_pc = 7;
    let good_hash = [0xbb; 16];
    let good_head_pc = 3;
    let installable: Vec<Installable> = vec![
        (0, bail_hash, bail_head_pc, bailing, ct.clone()),
        (1, good_hash, good_head_pc, good, ct),
    ];

    let mut module = build_object_module(&target).expect("object module");
    let (blob, metas) = lower_and_encode_meta(&mut module, &installable, LuaVersion::Lua55, false);
    assert_eq!(metas.len(), 1, "only the closed trace lowers");
    emit_meta_sections(&mut module, blob, &metas).expect("emit meta sections");
    let obj = module.finish().emit().expect("emit object");

    let meta = meta_section_bytes(&obj);
    assert_eq!(meta.len(), 48, "one 48-byte meta entry");
    assert_eq!(
        meta[0..16],
        good_hash,
        "entry carries the lowered trace's hash"
    );
    assert_eq!(
        u32::from_le_bytes(meta[16..20].try_into().unwrap()),
        good_head_pc,
        "entry carries the lowered trace's head pc"
    );
}
