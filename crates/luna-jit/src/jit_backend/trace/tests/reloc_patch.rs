//! The addresses a trace's code holds for one Vm, rewritten for another:
//! the baseline tier's code and the Cranelift code replayed from it give
//! the new values once their relocation sites are patched.

use super::super::lir::{self, CodeArena, Lir};
use super::super::reloc::{self, Code};
use super::*;
use cranelift_codegen::ir::MemFlagsData;

const A: [i64; 2] = [0x0000_7f12_3456_7890, 0x0000_5555_0000_1234];
const B: [i64; 2] = [0x0000_6abc_def0_1111, 0x0000_7777_2222_3333];

/// Stores reloc 0, reloc 1 and reloc 0 again plus reloc 1 at words 8..11.
fn build(l: &mut Lir) {
    let entry = l.create_block();
    l.append_block_params_for_function_params(entry);
    l.switch_to_block(entry);
    l.seal_block(entry);
    let p = Ins::block_params(l, entry)[0];
    let x = l.reloc(RelocKind::Str, A[0]);
    let y = l.reloc(RelocKind::Proto, A[1]);
    let x2 = l.reloc(RelocKind::Str, A[0]);
    let s = l.iadd(x2, y);
    l.store(MemFlagsData::trusted(), x, p, 64);
    l.store(MemFlagsData::trusted(), y, p, 72);
    l.store(MemFlagsData::trusted(), s, p, 80);
    let zero = l.iconst(types::I64, 0);
    l.return_(&[zero]);
}

fn run(entry: *const u8) -> [i64; 3] {
    let mut buf = [0i64; 12];
    // SAFETY: `entry` is code of the `TraceFn` ABI that only writes words
    // 8..11 of the buffer it is handed
    unsafe {
        let f = std::mem::transmute::<*const u8, TraceFn>(entry);
        f(buf.as_mut_ptr());
    }
    [buf[8], buf[9], buf[10]]
}

fn want(v: [i64; 2]) -> [i64; 3] {
    [v[0], v[1], v[0].wrapping_add(v[1])]
}

#[test]
fn relocated_code_holds_the_new_addresses() {
    let mut l = Lir::new();
    build(&mut l);
    assert_eq!(l.relocs.len(), 2, "one relocation per address");
    let mut arena = CodeArena::default();
    let (entry, code) = lir::assemble(&l, &mut arena, true).expect("baseline");
    let code: Code = code.expect("captured");
    assert_eq!(run(entry), want(A));
    let placed = arena.place(&code.relocated(&B)).expect("place");
    assert_eq!(run(placed), want(B), "baseline code");

    // the optimizing tier's code, compiled from the same instructions
    let mut module = build_trace_jit_module().expect("trace module");
    let id = lir::define_clif(&l, &l.relocs, &mut module, false).expect("replay");
    module.finalize_definitions().expect("finalize");
    let f = module.get_finalized_function(id);
    assert_eq!(run(f), want(A));
    let (len, sites) = reloc::take_sites().expect("sites");
    // SAFETY: `f..f + len` is the function just finalized in `module`
    let code = unsafe { reloc::copy_code(f, len, sites) };
    let placed = arena.place(&code.relocated(&B)).expect("place");
    assert_eq!(run(placed), want(B), "Cranelift code");
}
