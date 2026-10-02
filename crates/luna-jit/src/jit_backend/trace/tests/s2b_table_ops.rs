use super::*;
use crate::jit_backend::enter_jit;
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

/// Run a compiled trace under an `enter_jit` guard so the
/// `luna_jit_*` helpers can reach `vm` via the `JIT_VM`
/// thread-local.
///
/// SAFETY: `state.len() >= proto.max_stack` is the caller's
/// invariant — every helper that loads a table-typed slot
/// dereferences the i64 pointer there, so it must be a real
/// `Gc<Table>::as_ptr()` (or a `NewTable` op writes one).
fn run_trace(vm: &mut Vm, ct: &CompiledTrace, state: &mut [i64]) -> i64 {
    let _guard = enter_jit(vm, None);
    unsafe { (ct.entry)(state.as_mut_ptr()) }
}

#[test]
fn new_table_writes_non_null_table_ptr_into_dst() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // Trace: R[0] = {}. The B/C size hints don't matter — the
    // lowerer reaches for the unsized helper.
    let rec = closed_record(p, 0, &[Inst::iabc(Op::NewTable, 0, 0, 0, false)]);
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    let r = run_trace(&mut vm, &ct, &mut state);
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
    assert!(
        state[0] != 0,
        "NewTable must return a non-null Gc<Table> ptr"
    );
    assert!(vm.jit.pending_err.is_none(), "no deopt expected");
}

#[test]
fn set_i_then_get_i_roundtrips_through_a_fresh_table() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = {}
    // R[0][1] = R[2]      (SetI with B=1 immediate key)
    // R[3] = R[0][1]       (GetI with C=1 immediate key)
    let rec = closed_record(
        p,
        0,
        &[
            Inst::iabc(Op::NewTable, 0, 0, 0, false),
            Inst::iabc(Op::SetI, 0, 1, 2, false),
            Inst::iabc(Op::GetI, 3, 0, 1, false),
        ],
    );
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[2] = 42; // value to write
    let r = run_trace(&mut vm, &ct, &mut state);
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
    assert!(vm.jit.pending_err.is_none(), "no metatable → no deopt");
    assert_eq!(state[3], 42, "Get must see the value Set wrote");
}

#[test]
fn len_reports_array_size_after_set_i_sequence() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // R[0] = {}
    // R[0][1] = R[1]
    // R[0][2] = R[1]
    // R[0][3] = R[1]
    // R[2] = #R[0]
    let rec = closed_record(
        p,
        0,
        &[
            Inst::iabc(Op::NewTable, 0, 0, 0, false),
            Inst::iabc(Op::SetI, 0, 1, 1, false),
            Inst::iabc(Op::SetI, 0, 2, 1, false),
            Inst::iabc(Op::SetI, 0, 3, 1, false),
            Inst::iabc(Op::Len, 2, 0, 0, false),
        ],
    );
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[1] = 99;
    let r = run_trace(&mut vm, &ct, &mut state);
    assert_eq!(crate::jit_backend::trace::exit_pc(r), 0);
    assert_eq!(state[2], 3, "Len must report array length 3");
}

/// A table with a metatable triggers the helper's
/// `jit_pending_err` short-circuit (PUC routes the write through
/// `__newindex`; the helper bypasses it, so the lowerer must
/// deopt instead).
/// A store into a table with a metatable is left to the interpreter:
/// the trace exits at that store, after doing the ones before it
/// once, with nothing parked for the dispatcher.
#[test]
fn metatable_on_set_i_exits_at_the_store() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let plain = vm.heap.new_table();
    let t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    unsafe { t.as_mut() }.set_metatable(Some(mt));

    // R[0][1] = R[1]; R[2][1] = R[1]
    let mut rec = closed_record(
        p,
        0,
        &[
            Inst::iabc(Op::SetI, 0, 1, 1, false),
            Inst::iabc(Op::SetI, 2, 1, 1, false),
        ],
    );
    rec.entry_tags[0] = luna_core::runtime::value::raw::TABLE;
    rec.entry_tags[2] = luna_core::runtime::value::raw::TABLE;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = plain.as_ptr() as i64;
    state[1] = 7;
    state[2] = t.as_ptr() as i64;
    let r = run_trace(&mut vm, &ct, &mut state);

    assert_eq!(crate::jit_backend::trace::exit_pc(r), 1);
    assert!(vm.jit.pending_err.is_none());
    assert!(matches!(
        plain.get_int(1),
        luna_core::runtime::Value::Int(7)
    ));
    assert!(t.get_int(1).is_nil());
    assert_eq!(vm.jit.counters.deopt, 1);
}

#[test]
fn metatable_on_get_i_parks_pending_err() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    unsafe { t.as_mut() }.set_metatable(Some(mt));

    // Trace: R[1] = R[0][1].
    let mut rec = closed_record(p, 0, &[Inst::iabc(Op::GetI, 1, 0, 1, false)]);
    rec.entry_tags[0] = luna_core::runtime::value::raw::TABLE;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = t.as_ptr() as i64;
    run_trace(&mut vm, &ct, &mut state);

    assert!(vm.jit.pending_err.is_some(), "GetI deopt on metatable");
}

#[test]
fn metatable_on_len_exits_at_the_len() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let plain = vm.heap.new_table();
    let t = vm.heap.new_table();
    let mt = vm.heap.new_table();
    unsafe { t.as_mut() }.set_metatable(Some(mt));

    // R[0][1] = R[1]; R[3] = #R[2]
    let mut rec = closed_record(
        p,
        0,
        &[
            Inst::iabc(Op::SetI, 0, 1, 1, false),
            Inst::iabc(Op::Len, 3, 2, 0, false),
        ],
    );
    rec.entry_tags[0] = luna_core::runtime::value::raw::TABLE;
    rec.entry_tags[2] = luna_core::runtime::value::raw::TABLE;
    let ct = try_compile_trace(vm.jit.storage.as_mut(), &rec).expect("compile");

    let mut state: Vec<i64> = vec![0; p.max_stack as usize];
    state[0] = plain.as_ptr() as i64;
    state[1] = 7;
    state[2] = t.as_ptr() as i64;
    state[3] = 42;
    let r = run_trace(&mut vm, &ct, &mut state);

    assert_eq!(crate::jit_backend::trace::exit_pc(r), 1);
    assert!(vm.jit.pending_err.is_none());
    assert!(matches!(
        plain.get_int(1),
        luna_core::runtime::Value::Int(7)
    ));
    assert_eq!(state[3], 42, "the length was not written");
}

#[test]
fn new_table_dst_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    // Reg index 200 is way past the proto's max_stack (~5).
    let rec = closed_record(p, 0, &[Inst::iabc(Op::NewTable, 200, 0, 0, false)]);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn get_i_table_reg_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let rec = closed_record(p, 0, &[Inst::iabc(Op::GetI, 0, 200, 1, false)]);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn set_i_value_reg_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let rec = closed_record(p, 0, &[Inst::iabc(Op::SetI, 0, 1, 200, false)]);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}

#[test]
fn len_table_reg_out_of_bounds_bails() {
    let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
    let p = load_proto(&mut vm, WIDE_SRC);
    let rec = closed_record(p, 0, &[Inst::iabc(Op::Len, 0, 200, 0, false)]);
    assert!(try_compile_trace(vm.jit.storage.as_mut(), &rec).is_none());
}
