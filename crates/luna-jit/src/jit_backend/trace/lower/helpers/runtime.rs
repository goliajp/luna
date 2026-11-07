//! The helpers for string buffers, upvalues, math folds and side exits.

use super::*;

/// `luna_jit_*` helpers for string buffers, upvalues, math folds and side exits.
#[derive(Clone, Copy)]
pub(in crate::jit_backend::trace::lower) struct RuntimeHelpers {
    pub(in crate::jit_backend::trace::lower) str_buf_acquire_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_buf_release_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_buf_extend_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_buf_intern_id: FuncId,
    pub(in crate::jit_backend::trace::lower) get_int_id: FuncId,
    pub(in crate::jit_backend::trace::lower) suppress_admit_id: FuncId,
    pub(in crate::jit_backend::trace::lower) gc_due_id: FuncId,
    pub(in crate::jit_backend::trace::lower) sunk_table_id: FuncId,
    pub(in crate::jit_backend::trace::lower) sunk_grow_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_buf_concat_id: FuncId,
    pub(in crate::jit_backend::trace::lower) math_fn_check_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_sub_id: FuncId,
    pub(in crate::jit_backend::trace::lower) str_sub_checked_id: FuncId,
    pub(in crate::jit_backend::trace::lower) len_checked_id: FuncId,
    pub(in crate::jit_backend::trace::lower) upval_get_id: FuncId,
    pub(in crate::jit_backend::trace::lower) upval_get_checked_id: FuncId,
    /// `luna_jit_upval_of_checked(cl, idx, want, out)`: an upvalue of an
    /// inlined function's closure
    pub(in crate::jit_backend::trace::lower) upval_of_checked_id: FuncId,
    /// `luna_jit_op_self_checked(t, key, want, out)`: a method lookup
    pub(in crate::jit_backend::trace::lower) op_self_checked_id: FuncId,
    pub(in crate::jit_backend::trace::lower) head_closure_id: FuncId,
    pub(in crate::jit_backend::trace::lower) materialize_id: FuncId,
    pub(in crate::jit_backend::trace::lower) mat_sunk_id: FuncId,
    /// `luna_jit_side_cell_list(list, reg_state) -> ok`: a side cell's
    /// checks or stores (see `side_call`)
    pub(in crate::jit_backend::trace::lower) side_cell_list_id: FuncId,
}

/// An imported helper taking `n_params` `i64`s and returning one.
pub(super) fn declare_i64_import<E: Emit>(
    bcx: &mut E,
    name: &str,
    n_params: usize,
) -> Option<FuncId> {
    let mut sig = bcx.make_signature();
    for _ in 0..n_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    bcx.declare_function(name, Linkage::Import, &sig).ok()
}

/// An imported helper taking `n_params` `i64`s and returning nothing.
pub(super) fn declare_void_import<E: Emit>(
    bcx: &mut E,
    name: &str,
    n_params: usize,
) -> Option<FuncId> {
    let mut sig = bcx.make_signature();
    for _ in 0..n_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    bcx.declare_function(name, Linkage::Import, &sig).ok()
}

pub(super) fn declare_runtime_helpers<E: Emit>(bcx: &mut E) -> Option<RuntimeHelpers> {
    // `fn luna_jit_str_buf_acquire() -> i64`.
    // Returns a `*mut Vec<u8>` (boxed-leaked); used by buffered
    // accumulator emit at trace fn entry.
    let str_buf_acquire_id = declare_i64_import(bcx, "luna_jit_str_buf_acquire", 0)?;

    // `fn luna_jit_str_buf_release(buf: i64)`.
    let str_buf_release_id = declare_void_import(bcx, "luna_jit_str_buf_release", 1)?;

    // `fn luna_jit_str_buf_extend(buf, str_ptr) -> i64`.
    let str_buf_extend_id = declare_i64_import(bcx, "luna_jit_str_buf_extend", 2)?;

    // `fn luna_jit_str_buf_intern(buf, slot) -> i64`.
    let str_buf_intern_id = declare_i64_import(bcx, "luna_jit_str_buf_intern", 2)?;
    // Squelch unused warnings.
    let _ = (
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
    );

    let get_int_id = declare_i64_import(bcx, "luna_jit_table_get_int", 2)?;

    let suppress_admit_id = bcx
        .declare_function(
            "luna_jit_suppress_trace_admit",
            Linkage::Import,
            &bcx.make_signature(),
        )
        .ok()?;
    let gc_due_id = declare_i64_import(bcx, "luna_jit_gc_due", 3)?;
    let sunk_table_id = declare_i64_import(bcx, "luna_jit_sunk_table", 1)?;
    let sunk_grow_id = declare_i64_import(bcx, "luna_jit_sunk_grow", 2)?;
    let str_buf_concat_id = declare_i64_import(bcx, "luna_jit_str_buf_concat", 3)?;
    let math_fn_check_id = declare_i64_import(bcx, "luna_jit_math_fn_is_library", 2)?;

    let str_sub_id = declare_i64_import(bcx, "luna_jit_str_sub", 3)?;
    let str_sub_checked_id = declare_i64_import(bcx, "luna_jit_str_sub_checked", 3)?;

    let len_checked_id = declare_i64_import(bcx, "luna_jit_table_len_checked", 1)?;

    // `fn luna_jit_upval_get(idx: i64) -> i64`: the raw payload of
    // `JIT_CL`'s upvalue `idx`, without its tag (exit tags carry the
    // entry tag through)
    let mut upval_get_sig = bcx.make_signature();
    upval_get_sig.params.push(AbiParam::new(types::I64));
    upval_get_sig.returns.push(AbiParam::new(types::I64));
    let upval_get_checked_id = declare_i64_import(bcx, "luna_jit_upval_get_checked", 3)?;
    let upval_of_checked_id = declare_i64_import(bcx, "luna_jit_upval_of_checked", 4)?;
    let op_self_checked_id = declare_i64_import(bcx, "luna_jit_op_self_checked", 4)?;
    let upval_get_id = bcx
        .declare_function("luna_jit_upval_get", Linkage::Import, &upval_get_sig)
        .ok()?;
    let head_closure_id = declare_i64_import(bcx, "luna_jit_head_closure", 0)?;

    // `fn luna_jit_trace_materialize_frames(n: u64,
    // metas: *const FrameMaterializeInfo, closures: *const i64) -> i64`. Called by the
    // lowerer's cmp@d>0 emit.
    let materialize_id = declare_i64_import(bcx, "luna_jit_trace_materialize_frames", 3)?;

    // `fn luna_jit_materialize_sunk_table(cap: i64,
    // raws_ptr: *const u64, kinds_ptr: *const u8) -> i64`. Emit
    // per cmp side-exit per live Sinkable site: stack-allocates
    // a `cap × 8` raws buffer + a `cap × 1` kinds buffer, fills
    // them from the site's virt slot Variables + virt_kinds tracker,
    // calls this helper, writes the returned `Value::Table` raw
    // bits into the slot's regs Variable so the subsequent
    // `store_back` lands the heap pointer in `reg_state[a]`.
    // 7 i64 args:
    //   cap, arr_raws, arr_kinds, n_hash, hash_keys, hash_raws, hash_kinds
    // Returns: heap table raw payload (i64 Gc<Table> ptr).
    let mat_sunk_id = declare_i64_import(bcx, "luna_jit_materialize_sunk_table", 8)?;
    let side_cell_list_id = declare_i64_import(bcx, "luna_jit_side_cell_list", 2)?;

    Some(RuntimeHelpers {
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
        get_int_id,
        suppress_admit_id,
        gc_due_id,
        sunk_table_id,
        sunk_grow_id,
        str_buf_concat_id,
        math_fn_check_id,
        str_sub_id,
        str_sub_checked_id,
        len_checked_id,
        upval_get_id,
        upval_get_checked_id,
        upval_of_checked_id,
        op_self_checked_id,
        head_closure_id,
        materialize_id,
        mat_sunk_id,
        side_cell_list_id,
    })
}
