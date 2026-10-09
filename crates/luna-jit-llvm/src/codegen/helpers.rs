//! The `luna_jit_*` helper table the chunk lowerer calls.

use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::values::FunctionValue;
use std::collections::HashMap;

/// `luna_jit_*` helper registry. Each tuple is
/// `(symbol_name, fn_ptr_as_usize, n_args, returns_i64)`. The full
/// 27-entry set from `luna-jit-helpers` lives here so future ops can
/// reach for any helper without re-touching the registration path;
/// the inkwell `add_function` + `ExecutionEngine::add_global_mapping`
/// pair fires unconditionally on every compute-path compile. LLVM
/// will strip the unreferenced declarations at JIT time (zero cost
/// when only a subset is actually called).
///
/// Pointer-typed helper args (`*const u8`, `*mut i64`, …) are declared
/// as `i64` because:
/// 1. luna-jit-helpers' own bodies bit-cast `i64` → `*const _` at
///    the boundary; Cranelift's `build_jit_module_with_helpers`
///    mirrors this (all `AbiParam::new(types::I64)`).
/// 2. LLVM 16+ uses opaque pointers — passing `i64` through the
///    C-ABI and bitcast'ing on the helper side is layout-equivalent
///    to passing a `ptr` directly on 64-bit targets.
/// 3. Keeps the registry one-liner per helper.
type Helper = (&'static str, usize, u32, bool);

/// `h!(helper, n_args, returns_i64)`: one registry entry.
macro_rules! h {
    ($f:ident, $n:expr, $r:expr) => {
        (stringify!($f), $f as *const () as usize, $n, $r)
    };
}

fn helper_registry() -> Vec<Helper> {
    use luna_jit_helpers::{
        luna_jit_enter_ctx, luna_jit_materialize_sunk_table, luna_jit_new_table,
        luna_jit_new_table_sized, luna_jit_no_deopt_parked, luna_jit_op_close, luna_jit_op_closure,
        luna_jit_op_concat, luna_jit_op_get_tab_up, luna_jit_op_tforcall, luna_jit_park_deopt,
        luna_jit_self_call_slow, luna_jit_self_upval_check, luna_jit_spill_to_stack,
        luna_jit_stack_load, luna_jit_stack_tag, luna_jit_stack_update_raw,
        luna_jit_str_buf_acquire, luna_jit_str_buf_extend, luna_jit_str_buf_intern,
        luna_jit_str_buf_release, luna_jit_table_get_field, luna_jit_table_get_float,
        luna_jit_table_get_int, luna_jit_table_len, luna_jit_table_set_field,
        luna_jit_table_set_float_float, luna_jit_table_set_int, luna_jit_table_set_nil,
        luna_jit_table_set_raw, luna_jit_trace_materialize_frames, luna_jit_upval_get,
        luna_jit_upval_is_int,
    };
    vec![
        h!(luna_jit_new_table, 0, true),
        h!(luna_jit_new_table_sized, 1, true),
        h!(luna_jit_materialize_sunk_table, 8, true),
        h!(luna_jit_table_set_int, 3, false),
        h!(luna_jit_table_set_raw, 4, false),
        h!(luna_jit_table_set_field, 4, false),
        h!(luna_jit_table_get_field, 2, true),
        h!(luna_jit_op_get_tab_up, 2, true),
        h!(luna_jit_table_set_nil, 2, false),
        h!(luna_jit_table_set_float_float, 3, false),
        h!(luna_jit_table_get_int, 2, true),
        h!(luna_jit_table_get_float, 2, true),
        h!(luna_jit_upval_get, 1, true),
        h!(luna_jit_upval_is_int, 1, true),
        h!(luna_jit_no_deopt_parked, 0, true),
        h!(luna_jit_park_deopt, 0, false),
        h!(luna_jit_enter_ctx, 1, false),
        h!(luna_jit_self_call_slow, 7, true),
        h!(luna_jit_self_upval_check, 1, true),
        h!(luna_jit_op_close, 1, true),
        h!(luna_jit_stack_update_raw, 2, false),
        h!(luna_jit_op_concat, 3, true),
        h!(luna_jit_str_buf_acquire, 0, true),
        h!(luna_jit_str_buf_release, 1, false),
        h!(luna_jit_str_buf_extend, 2, true),
        h!(luna_jit_str_buf_intern, 1, true),
        h!(luna_jit_op_tforcall, 6, true),
        h!(luna_jit_stack_load, 1, true),
        h!(luna_jit_stack_tag, 1, true),
        h!(luna_jit_spill_to_stack, 3, false),
        h!(luna_jit_op_closure, 1, true),
        h!(luna_jit_trace_materialize_frames, 2, true),
        h!(luna_jit_table_len, 1, true),
    ]
}

/// Declare every `luna_jit_*` helper in `module`
/// as an external function. The returned map lets emit sites grab the
/// `FunctionValue` for `build_call` without re-looking-up via
/// `Module::get_function` per call. Pairs with [`bind_helper_symbols`]
/// which fires after `create_jit_execution_engine` to wire the IR
/// declarations to their actual Rust function addresses.
pub(crate) fn declare_jit_helpers<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> HashMap<&'static str, FunctionValue<'ctx>> {
    let i64_ty = ctx.i64_type();
    let void_ty = ctx.void_type();
    let mut map = HashMap::with_capacity(27);
    for (name, _addr, n_args, returns_i64) in helper_registry() {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum> =
            (0..n_args).map(|_| i64_ty.into()).collect();
        let fn_ty = if returns_i64 {
            i64_ty.fn_type(&params, false)
        } else {
            void_ty.fn_type(&params, false)
        };
        let fn_val = module.add_function(name, fn_ty, Some(Linkage::External));
        map.insert(name, fn_val);
    }
    map
}

/// Bind every declared `luna_jit_*` helper to its
/// Rust function address via `ExecutionEngine::add_global_mapping`.
/// Without this step the JIT'd mcode would call undefined external
/// symbols at run time (LLVM's default resolver tries `dlsym` first,
/// and luna's `extern "C"` helpers are `#[unsafe(no_mangle)]`
/// so they ARE dlsym-able when luna is loaded as a `dylib`/`cdylib`
/// — but the rlib link path strips them, exactly mirroring Cranelift's
/// `JITBuilder::symbol` rationale in `build_jit_module_with_helpers`).
///
/// Looks the declarations up in `module` by name: the optimizer deletes
/// the ones nothing calls, so handles kept from before it ran may point at
/// freed functions.
pub(super) fn bind_helper_symbols<'ctx>(
    engine: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    module: &Module<'ctx>,
) {
    for (name, addr, _n_args, _returns_i64) in helper_registry() {
        if let Some(fn_val) = module.get_function(name) {
            engine.add_global_mapping(&fn_val, addr);
        }
    }
}
