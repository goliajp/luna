use super::*;

/// The Vm's JIT storage, for a test to compile a trace into.
fn vm_storage(vm: &mut luna_core::vm::Vm) -> &mut dyn luna_core::__private::JitStorage {
    use luna_core::__private::VmJitState;
    // SAFETY: the tests only compile into the storage, which stays in place
    unsafe { vm.jit_mut() }.storage.as_mut()
}

mod div_magic;
mod drop_unused_block_params_tests;
mod lir_primitives;
mod reloc_patch;
mod s13a_depth_invariant;
mod s14b_v0_scaffolding;
mod s2b_arith;
mod s2b_call_truncation;
mod s2b_cmp;
mod s2b_table_ops;
mod s4_step3a_op_offsets;
mod s4_step3b_inline_emit;
mod s4_step4b_skeleton;
mod s6_step_a1;
mod share_content;
