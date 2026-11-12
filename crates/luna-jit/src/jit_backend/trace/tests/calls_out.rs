//! `Lir::calls_out`: what makes a trace's code run in the compiler's scope
//! when it runs from its parent's exit (`CompiledTraceParts::calls_helpers`).
//! Each kind of call the lowerer emits sets it; nothing else does.

use super::super::emit::{Emit, Ins};
use super::super::lir::Lir;
use cranelift_codegen::ir::{BlockArg, types};
use cranelift_module::Linkage;

/// A function with a block of `iconst`s and a jump: no call.
fn plain() -> Lir {
    let mut l = Lir::new();
    let b = l.create_block();
    let c = l.create_block();
    l.switch_to_block(b);
    let x = l.iconst(types::I64, 3);
    l.jump(c, &[BlockArg::Value(x)]);
    l
}

#[test]
fn each_kind_of_call_calls_out() {
    assert!(!plain().calls_out());
    // a helper, by its declared name
    let mut l = plain();
    let sig = l.make_signature();
    let id = l
        .declare_function("luna_jit_op_tforcall", Linkage::Import, &sig)
        .unwrap();
    let f = l.import_func(id);
    l.call(f, &[]);
    assert!(l.calls_out(), "call");
    // a function at an address
    let mut l = plain();
    let sig = l.make_signature();
    let s = l.import_signature(sig);
    let at = l.iconst(types::I64, 0);
    l.call_indirect(s, at, &[]);
    assert!(l.calls_out(), "call_indirect");
    // a side trace from a side cell
    let mut l = plain();
    let call = l.create_block();
    let z = l.iconst(types::I64, 0);
    let args = [BlockArg::Value(z), BlockArg::Value(z), BlockArg::Value(z)];
    l.calling_exit(0, call, &args);
    assert!(l.calls_out(), "calling_exit");
    // taken again from the spare buffers: cleared
    l.give();
    assert!(!Lir::take().calls_out());
}
