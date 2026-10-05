use super::*;

/// build a fresh `JITModule` configured with
/// all `luna_jit_*` helper symbols pre-registered. Shared by the
/// runtime JIT entry [`try_compile_int_chunk`] and tests; the AOT
/// pipeline (luna-aot) builds an `ObjectModule` instead and feeds it
/// to the same [`lower_int_chunk_into`] generic body.
fn build_jit_module_with_helpers() -> Option<JITModule> {
    let mut builder =
        JITBuilder::with_isa(method_isa()?, cranelift_module::default_libcall_names());
    builder.memory_provider(Box::new(code_memory::CodeMemory::new()));
    // register the Rust helpers so the cranelift JIT can resolve them at
    // finalize time: executables that link luna as an rlib strip the
    // `#[no_mangle]` symbols, and the default `dlsym(RTLD_DEFAULT)` resolver
    // then fails. The libm symbols the math folds use (`sin`, `cos`, …) are
    // linked from libc and stay resolvable via dlsym. A lookup function
    // rather than one `symbol` entry each: entries are owned strings in a
    // map built anew for every module.
    builder.symbol_lookup_fn(Box::new(method_helper));
    Some(JITModule::new(builder))
}

/// The method JIT's target, built once: the flags never change, and
/// building it per function cost about as much as compiling a small one.
fn method_isa() -> Option<cranelift_codegen::isa::OwnedTargetIsa> {
    static ISA: std::sync::OnceLock<Option<cranelift_codegen::isa::OwnedTargetIsa>> =
        std::sync::OnceLock::new();
    ISA.get_or_init(|| {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").ok();
        flag_builder.set("is_pic", "false").ok();
        flag_builder.set("opt_level", "speed").ok();
        // Release builds leave the IR verifier out, as the trace JIT does
        // (see `build_trace_jit_module`).
        if !cfg!(debug_assertions) {
            flag_builder.set("enable_verifier", "false").ok();
        }
        cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flag_builder))
            .ok()
    })
    .clone()
}

/// The address of a Rust helper the method JIT's code calls.
fn method_helper(name: &str) -> Option<*const u8> {
    Some(match name {
        "luna_jit_new_table" => luna_jit_new_table as *const u8,
        "luna_jit_new_table_sized" => luna_jit_new_table_sized as *const u8,
        "luna_jit_table_set_int" => luna_jit_table_set_int as *const u8,
        "luna_jit_table_set_float_float" => luna_jit_table_set_float_float as *const u8,
        "luna_jit_table_set_raw" => luna_jit_table_set_raw as *const u8,
        "luna_jit_table_get_int" => luna_jit_table_get_int as *const u8,
        "luna_jit_table_get_float" => luna_jit_table_get_float as *const u8,
        "luna_jit_table_len" => luna_jit_table_len as *const u8,
        "luna_jit_upval_get" => luna_jit_upval_get as *const u8,
        "luna_jit_upval_get_float" => luna_jit_upval_get_float as *const u8,
        "luna_jit_self_upval_check" => luna_jit_self_upval_check as *const u8,
        "luna_jit_math_fn_is_library" => luna_jit_math_fn_is_library as *const u8,
        "luna_jit_park_deopt" => luna_jit_park_deopt as *const u8,
        "luna_jit_enter_ctx" => luna_jit_enter_ctx as *const u8,
        "luna_jit_self_call_slow" => luna_jit_self_call_slow as *const u8,
        "luna_jit_table_get_int_checked" => luna_jit_table_get_int_checked as *const u8,
        "luna_jit_table_get_float_checked" => luna_jit_table_get_float_checked as *const u8,
        _ => return super::trace::reloc::resolve_symbol(name),
    })
}

/// Try to JIT-compile `proto`. Returns `None` when any opcode in the
/// body falls outside the cumulative whitelist — the interpreter then
/// handles the chunk unchanged. `pre53` (Lua 5.1 / 5.2 / 5.3) selects
/// the pre-5.3 `ForPrep` / `ForLoop` form; pass `false` (Lua 5.4 /
/// 5.5) for the counted-loop form. The dialect bit also participates
/// in the cache key — see `proto_cache_key`.
///
/// thin wrapper around the backend-agnostic
/// [`lower_int_chunk_into`] generic; constructs a `JITModule`,
/// finalizes the compiled fn into RWX memory, and wraps the entry ptr
/// in a [`JitHandle`] that owns the module for the entry's lifetime.
pub fn try_compile_int_chunk(proto: Gc<Proto>, pre53: bool, float_only: bool) -> Option<JitHandle> {
    let mut module = send_jit_module::UnpublishedModule::new(build_jit_module_with_helpers()?);
    let (fn_id, meta) = lower_int_chunk_into(&mut *module, proto, pre53, float_only)?;
    module.finalize_definitions().ok()?;
    chunk_share::count_codegen();

    // `LUNA_JIT_TRACE=1` prints one line per
    // successful JIT compile with the Proto's source location +
    // signature. A regression in
    // (e.g.) errors.lua can grep this trace to pinpoint the
    // exact `load(...)` snippet that JIT'd, instead of bisecting
    // by hand. The check is one TLS read per compile when the
    // env var is unset — negligible vs the cranelift codegen
    // cost.
    if std::env::var_os("LUNA_JIT_TRACE").is_some() {
        let src_bytes = proto.source.as_bytes();
        let src = std::str::from_utf8(src_bytes).unwrap_or("<non-utf8 source>");
        let line_start = proto.line_defined;
        let line_end = proto.last_line_defined;
        let ChunkMeta {
            num_args,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
            ..
        } = meta;
        eprintln!(
            "[luna jit] {src}:{line_start}-{line_end} params={} code_len={} num_args={num_args} arg_float_mask={arg_float_mask:#x} arg_table_mask={arg_table_mask:#x} ret_is_float={ret_is_float} ret_is_table={ret_is_table}",
            proto.num_params,
            proto.code.len(),
        );
    }

    let ptr = module.get_finalized_function(fn_id);
    Some(JitHandle {
        // wrap with the `SendJitModule` sleeve
        _module: module.publish(),
        entry_raw: ptr,
        num_args: meta.num_args,
        returns_one: meta.returns_one,
        arg_float_mask: meta.arg_float_mask,
        arg_table_mask: meta.arg_table_mask,
        ret_is_float: meta.ret_is_float,
        ret_is_table: meta.ret_is_table,
    })
}
