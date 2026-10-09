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
    Some(match name {
        "luna_jit_new_table_sized" => crate::jit_backend::luna_jit_new_table_sized as *const u8,
        "luna_jit_table_reserve_list" => {
            crate::jit_backend::luna_jit_table_reserve_list as *const u8
        }
        "luna_jit_table_set_int_checked" => {
            crate::jit_backend::luna_jit_table_set_int_checked as *const u8
        }
        "luna_jit_table_set_field_checked" => {
            crate::jit_backend::luna_jit_table_set_field_checked as *const u8
        }
        "luna_jit_table_set_checked" => crate::jit_backend::luna_jit_table_set_checked as *const u8,
        "luna_jit_table_get_field" => crate::jit_backend::luna_jit_table_get_field as *const u8,
        "luna_jit_op_get_tab_up" => crate::jit_backend::luna_jit_op_get_tab_up as *const u8,
        "luna_jit_table_get_int" => crate::jit_backend::luna_jit_table_get_int as *const u8,
        "luna_jit_table_get_int_checked" => {
            crate::jit_backend::luna_jit_table_get_int_checked as *const u8
        }
        "luna_jit_table_get_field_checked" => {
            crate::jit_backend::luna_jit_table_get_field_checked as *const u8
        }
        "luna_jit_op_get_tab_up_checked" => {
            crate::jit_backend::luna_jit_op_get_tab_up_checked as *const u8
        }
        "luna_jit_table_len_checked" => crate::jit_backend::luna_jit_table_len_checked as *const u8,
        "luna_jit_math_fn_is_library" => {
            crate::jit_backend::luna_jit_math_fn_is_library as *const u8
        }
        "luna_jit_str_sub" => crate::jit_backend::luna_jit_str_sub as *const u8,
        "luna_jit_fmod" => crate::jit_backend::luna_jit_fmod as *const u8,
        "luna_jit_math1" => crate::jit_backend::luna_jit_math1 as *const u8,
        "luna_jit_pow" => crate::jit_backend::luna_jit_pow as *const u8,
        "luna_jit_numpow" => crate::jit_backend::luna_jit_numpow as *const u8,
        "luna_jit_upval_get_checked" => crate::jit_backend::luna_jit_upval_get_checked as *const u8,
        "luna_jit_upval_of_checked" => crate::jit_backend::luna_jit_upval_of_checked as *const u8,
        "luna_jit_op_self_checked" => crate::jit_backend::luna_jit_op_self_checked as *const u8,
        "luna_jit_suppress_trace_admit" => {
            crate::jit_backend::luna_jit_suppress_trace_admit as *const u8
        }
        "luna_jit_upval_get" => crate::jit_backend::luna_jit_upval_get as *const u8,
        "luna_jit_head_closure" => crate::jit_backend::luna_jit_head_closure as *const u8,
        "luna_jit_trace_materialize_frames" => {
            crate::jit_backend::luna_jit_trace_materialize_frames as *const u8
        }
        "luna_jit_materialize_sunk_table" => {
            crate::jit_backend::luna_jit_materialize_sunk_table as *const u8
        }
        "luna_jit_op_closure" => crate::jit_backend::luna_jit_op_closure as *const u8,
        "luna_jit_op_closure_in" => crate::jit_backend::luna_jit_op_closure_in as *const u8,
        "luna_jit_set_top" => crate::jit_backend::luna_jit_set_top as *const u8,
        "luna_jit_spill_to_stack" => crate::jit_backend::luna_jit_spill_to_stack as *const u8,
        "luna_jit_op_close" => crate::jit_backend::luna_jit_op_close as *const u8,
        "luna_jit_op_tforcall" => crate::jit_backend::luna_jit_op_tforcall as *const u8,
        "luna_jit_stack_load" => crate::jit_backend::luna_jit_stack_load as *const u8,
        "luna_jit_stack_tag" => crate::jit_backend::luna_jit_stack_tag as *const u8,
        "luna_jit_op_concat" => crate::jit_backend::luna_jit_op_concat as *const u8,
        "luna_jit_stack_update_raw" => crate::jit_backend::luna_jit_stack_update_raw as *const u8,
        "luna_jit_str_buf_acquire" => crate::jit_backend::luna_jit_str_buf_acquire as *const u8,
        "luna_jit_str_buf_release" => crate::jit_backend::luna_jit_str_buf_release as *const u8,
        "luna_jit_str_buf_extend" => crate::jit_backend::luna_jit_str_buf_extend as *const u8,
        "luna_jit_str_buf_intern" => crate::jit_backend::luna_jit_str_buf_intern as *const u8,
        _ => return reloc::resolve_symbol(name),
    })
}
