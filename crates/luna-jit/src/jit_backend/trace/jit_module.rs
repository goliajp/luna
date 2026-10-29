//! The Cranelift module and target traces are compiled for, and the
//! helpers their code calls.

use super::*;

/// build a fresh `JITModule` configured with
/// every trace-side `luna_jit_*` helper symbol registered for
/// `Linkage::Import` resolution at finalize time.
///
/// Companion of [`crate::jit_backend::build_jit_module_with_helpers`] (the int-chunk
/// counterpart). The AOT pipeline (luna-aot) builds an `ObjectModule`
/// instead and resolves the same symbols at static-link time.
///
/// Both the int-chunk lowerer
/// ([`crate::jit_backend::lower_int_chunk_into`]) and the trace lowerer
/// ([`lower_trace_into`]) are fully generic over
/// `M: cranelift_module::Module`. The two emit-time helper free fns
/// ([`emit_table_set`] / [`emit_materialize_live_sunk`]) are also
/// generic. JIT-specific surfaces remaining are: this module-
/// construction helper, the JIT wrappers ([`try_compile_trace_with_options`]
/// + [`crate::jit_backend::try_compile_int_chunk`]) that finalize, and the
/// `TraceHandle` / `JitHandle` types that own the mmap'd `JITModule`
/// for the entry's lifetime. The trace lowerer returns a
/// `CompiledTrace` with [`placeholder_trace_fn`] in `entry`; the JIT
/// wrapper patches the real fn pointer after `finalize_definitions` +
/// `get_finalized_function`. The AOT pipeline (luna-aot) never
/// invokes the entry directly — it resolves the trace symbol at
/// static-link time and dispatches through its own table.
pub(super) fn build_trace_jit_module() -> Option<JITModule> {
    let mut builder = JITBuilder::with_isa(trace_isa()?, cranelift_module::default_libcall_names());
    builder.memory_provider(Box::new(crate::jit_backend::code_memory::CodeMemory::new()));
    // the lowerer's code calls the `luna_jit_*` helpers (see
    // `crate::jit_backend::build_jit_module_with_helpers` for why they are looked up
    // here rather than by dlsym)
    builder.symbol_lookup_fn(Box::new(trace_helper));
    Some(JITModule::new(builder))
}

/// The trace JIT's target, built once (see [`crate::jit_backend::method_isa`]).
pub(super) fn trace_isa() -> Option<cranelift_codegen::isa::OwnedTargetIsa> {
    static ISA: std::sync::OnceLock<Option<cranelift_codegen::isa::OwnedTargetIsa>> =
        std::sync::OnceLock::new();
    ISA.get_or_init(|| {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").ok()?;
        flag_builder.set("is_pic", "false").ok()?;
        // The egraph optimizer costs a fifth of a trace's compile time and
        // buys nothing on the code the lowerer emits. The single-pass register
        // allocator would halve compile time again, but its spills made a
        // numeric loop trace run 1.9x the instructions; traces keep the
        // backtracking one.
        flag_builder.set("opt_level", "none").ok()?;
        // the block offsets, for the loop head's alignment
        flag_builder.set("machine_code_cfg_info", "true").ok()?;
        // The IR verifier is a quarter of a trace's compile time (token_bucket:
        // 175 of 720 us of Cranelift passes). Release builds leave it out, as
        // wasmtime does; debug builds, which the lib tests run, keep it.
        if !cfg!(debug_assertions) {
            flag_builder.set("enable_verifier", "false").ok()?;
        }
        cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flag_builder))
            .ok()
    })
    .clone()
}

/// The address of a Rust helper trace code calls.
pub(super) fn trace_helper(name: &str) -> Option<*const u8> {
    // each helper by its name, as the trace's module imports it
    macro_rules! by_name {
        ($($f:ident),* $(,)?) => {
            match name {
                $(stringify!($f) => crate::jit_backend::$f as *const u8,)*
                _ => return reloc::resolve_symbol(name),
            }
        };
    }
    Some(by_name!(
        luna_jit_new_table_sized,
        luna_jit_table_reserve_list,
        luna_jit_table_set_int_checked,
        luna_jit_table_set_field_checked,
        luna_jit_table_set_checked,
        luna_jit_table_get_field,
        luna_jit_op_get_tab_up,
        luna_jit_table_get_int,
        luna_jit_table_get_int_checked,
        luna_jit_table_get_field_checked,
        luna_jit_op_get_tab_up_checked,
        luna_jit_table_len_checked,
        luna_jit_math_fn_is_library,
        luna_jit_str_sub,
        luna_jit_str_sub_checked,
        luna_jit_fmod,
        luna_jit_math1,
        luna_jit_pow,
        luna_jit_numpow,
        luna_jit_upval_get_checked,
        luna_jit_upval_of_checked,
        luna_jit_op_self_checked,
        luna_jit_gc_due,
        luna_jit_due_words,
        luna_jit_sunk_table,
        luna_jit_sunk_grow,
        luna_jit_str_buf_concat,
        luna_jit_suppress_trace_admit,
        luna_jit_upval_get,
        luna_jit_head_closure,
        luna_jit_trace_materialize_frames,
        luna_jit_materialize_sunk_table,
        luna_jit_op_closure,
        luna_jit_op_closure_in,
        luna_jit_set_top,
        luna_jit_spill_to_stack,
        luna_jit_op_close,
        luna_jit_op_tforcall,
        luna_jit_stack_load,
        luna_jit_stack_tag,
        luna_jit_op_concat,
        luna_jit_stack_update_raw,
        luna_jit_str_buf_acquire,
        luna_jit_str_buf_release,
        luna_jit_str_buf_extend,
        luna_jit_str_buf_intern,
    ))
}
