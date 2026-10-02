use super::*;

/// `luna_jit_*` helpers the per-op lowering calls: table access, closures, spills, `close`, generic-for and concat.
#[derive(Clone, Copy)]
pub(super) struct OpHelpers {
    pub(super) new_table_id: FuncId,
    pub(super) set_ids: StoreHelpers,
    pub(super) get_field_id: FuncId,
    pub(super) get_tab_up_id: FuncId,
    pub(super) get_field_checked_id: FuncId,
    pub(super) get_tab_up_checked_id: FuncId,
    pub(super) op_closure_id: FuncId,
    pub(super) spill_id: FuncId,
    pub(super) op_close_id: FuncId,
    pub(super) op_tforcall_id: FuncId,
    pub(super) stack_load_id: FuncId,
    pub(super) stack_tag_id: FuncId,
    pub(super) op_concat_id: FuncId,
}

/// `luna_jit_*` helpers for string buffers, upvalues, math folds and side exits.
#[derive(Clone, Copy)]
pub(super) struct RuntimeHelpers {
    pub(super) str_buf_acquire_id: FuncId,
    pub(super) str_buf_release_id: FuncId,
    pub(super) str_buf_extend_id: FuncId,
    pub(super) str_buf_intern_id: FuncId,
    pub(super) update_raw_id: FuncId,
    pub(super) get_int_id: FuncId,
    pub(super) suppress_admit_id: FuncId,
    pub(super) math_fn_check_id: FuncId,
    pub(super) str_sub_id: FuncId,
    pub(super) len_checked_id: FuncId,
    pub(super) upval_get_id: FuncId,
    pub(super) upval_get_checked_id: FuncId,
    pub(super) head_closure_id: FuncId,
    pub(super) materialize_id: FuncId,
    pub(super) mat_sunk_id: FuncId,
}

/// Every helper a trace calls, declared in `module` once per trace.
#[derive(Clone, Copy)]
pub(super) struct Helpers {
    pub(super) op: OpHelpers,
    pub(super) rt: RuntimeHelpers,
}

pub(super) fn declare_helpers<M: Module>(module: &mut M) -> Option<Helpers> {
    Some(Helpers {
        op: declare_op_helpers(module)?,
        rt: declare_runtime_helpers(module)?,
    })
}

fn declare_op_helpers<M: Module>(module: &mut M) -> Option<OpHelpers> {
    // `module` arrives as `&mut M` from the
    // caller. The JIT wrapper [`try_compile_trace_with_options`]
    // constructs a `JITModule` via [`build_trace_jit_module`]; the AOT
    // pipeline (luna-aot) feeds an `ObjectModule` of its own. The
    // helper-symbol contract is identical (both resolve `luna_jit_*` —
    // the JIT via `JITBuilder::symbol`, the AOT via static link).

    // Helper signatures — declared up front so emit can look them
    // up without re-declaring per call site. Unused declarations
    // get tree-shaken at optimization.
    let mut new_table_sig = module.make_signature();
    new_table_sig.returns.push(AbiParam::new(types::I64));
    let new_table_id = module
        .declare_function("luna_jit_new_table", Linkage::Import, &new_table_sig)
        .ok()?;

    // `fn luna_jit_table_set_{int,field}_checked(t, key, val_raw, val_tag)
    // -> stored` and `fn luna_jit_table_set_checked(t, key_raw, key_tag,
    // val_raw, val_tag) -> stored`
    let mut set_sig = module.make_signature();
    for _ in 0..4 {
        set_sig.params.push(AbiParam::new(types::I64));
    }
    set_sig.returns.push(AbiParam::new(types::I64));
    let mut set_any_sig = set_sig.clone();
    set_any_sig.params.push(AbiParam::new(types::I64));
    let set_ids = StoreHelpers {
        int_key: module
            .declare_function("luna_jit_table_set_int_checked", Linkage::Import, &set_sig)
            .ok()?,
        str_key: module
            .declare_function(
                "luna_jit_table_set_field_checked",
                Linkage::Import,
                &set_sig,
            )
            .ok()?,
        any_key: module
            .declare_function("luna_jit_table_set_checked", Linkage::Import, &set_any_sig)
            .ok()?,
    };

    // `fn luna_jit_table_get_field(t, key_ptr) -> raw`.
    let mut get_field_sig = module.make_signature();
    get_field_sig.params.push(AbiParam::new(types::I64));
    get_field_sig.params.push(AbiParam::new(types::I64));
    get_field_sig.returns.push(AbiParam::new(types::I64));
    let get_field_id = module
        .declare_function("luna_jit_table_get_field", Linkage::Import, &get_field_sig)
        .ok()?;

    // `fn luna_jit_op_get_tab_up(upval_idx, key_ptr) -> raw`.
    let mut get_tab_up_sig = module.make_signature();
    get_tab_up_sig.params.push(AbiParam::new(types::I64));
    get_tab_up_sig.params.push(AbiParam::new(types::I64));
    get_tab_up_sig.returns.push(AbiParam::new(types::I64));
    let get_tab_up_id = module
        .declare_function("luna_jit_op_get_tab_up", Linkage::Import, &get_tab_up_sig)
        .ok()?;

    // Checked table reads (`luna_jit_table_get_field_checked` et al.):
    // `fn(table_or_upval, key, want_tag, out: *mut i64) -> ok`.
    let mut get_checked_sig = module.make_signature();
    for _ in 0..4 {
        get_checked_sig.params.push(AbiParam::new(types::I64));
    }
    get_checked_sig.returns.push(AbiParam::new(types::I64));
    let get_field_checked_id = module
        .declare_function(
            "luna_jit_table_get_field_checked",
            Linkage::Import,
            &get_checked_sig,
        )
        .ok()?;
    let get_tab_up_checked_id = module
        .declare_function(
            "luna_jit_op_get_tab_up_checked",
            Linkage::Import,
            &get_checked_sig,
        )
        .ok()?;

    // `fn luna_jit_op_closure(proto_idx: i64) -> i64`.
    // Returns the new Gc<LuaClosure> raw payload bits.
    let mut op_closure_sig = module.make_signature();
    op_closure_sig.params.push(AbiParam::new(types::I64));
    op_closure_sig.returns.push(AbiParam::new(types::I64));
    let op_closure_id = module
        .declare_function("luna_jit_op_closure", Linkage::Import, &op_closure_sig)
        .ok()?;

    // `fn luna_jit_spill_to_stack(slot_offset, tag, raw_bits)`.
    // Writes vm.stack[base + slot_offset] = Value::pack(tag, raw).
    let mut spill_sig = module.make_signature();
    spill_sig.params.push(AbiParam::new(types::I64));
    spill_sig.params.push(AbiParam::new(types::I64));
    spill_sig.params.push(AbiParam::new(types::I64));
    let spill_id = module
        .declare_function("luna_jit_spill_to_stack", Linkage::Import, &spill_sig)
        .ok()?;

    // `fn luna_jit_op_close(start_offset: i64) -> i64`.
    // Returns 0 (continue) or 1 (deopt — handler would run or
    // pre-existing pending_err).
    let mut op_close_sig = module.make_signature();
    op_close_sig.params.push(AbiParam::new(types::I64));
    op_close_sig.returns.push(AbiParam::new(types::I64));
    let op_close_id = module
        .declare_function("luna_jit_op_close", Linkage::Import, &op_close_sig)
        .ok()?;

    // `fn luna_jit_op_tforcall(abs_offset, nvars,
    // ctrl_out: *mut i64, key_out: *mut i64, val_out: *mut i64) -> i64`.
    // Batched: helper fills the three out pointers with raw bits
    // of R[A+2] / R[A+4] / R[A+5] and returns R[A+4]'s tag byte
    // (0..=11) on success, -1 on deopt. Emit reads the buffer via
    // cranelift `stack_load` IR (skips per-slot `stack_load` /
    // `stack_tag` helper calls — 4 helpers per iter would be the
    // bottleneck).
    let mut op_tforcall_sig = module.make_signature();
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.params.push(AbiParam::new(types::I64));
    op_tforcall_sig.returns.push(AbiParam::new(types::I64));
    let op_tforcall_id = module
        .declare_function("luna_jit_op_tforcall", Linkage::Import, &op_tforcall_sig)
        .ok()?;

    // `fn luna_jit_stack_load(slot) -> i64` returns
    // raw bits of vm.stack[trace_head_frame.base + slot]. Used to
    // reload trace IR Variables after TForCall mutates vm.stack.
    let mut stack_load_sig = module.make_signature();
    stack_load_sig.params.push(AbiParam::new(types::I64));
    stack_load_sig.returns.push(AbiParam::new(types::I64));
    let stack_load_id = module
        .declare_function("luna_jit_stack_load", Linkage::Import, &stack_load_sig)
        .ok()?;

    // `fn luna_jit_stack_tag(slot) -> i64` returns
    // the raw::* tag byte of vm.stack[trace_head_frame.base + slot].
    // TForLoop tail emit dispatches on this to pick exit-on-Nil /
    // continue-on-Int / deopt-on-other.
    let mut stack_tag_sig = module.make_signature();
    stack_tag_sig.params.push(AbiParam::new(types::I64));
    stack_tag_sig.returns.push(AbiParam::new(types::I64));
    let stack_tag_id = module
        .declare_function("luna_jit_stack_tag", Linkage::Import, &stack_tag_sig)
        .ok()?;

    // `fn luna_jit_op_concat(a, n) -> i64`. Returns
    // 0 on success (result at vm.stack[base+a]) or -1 on deopt
    // (metamethod path, type error, length overflow,
    // pre-existing pending_err).
    let mut op_concat_sig = module.make_signature();
    op_concat_sig.params.push(AbiParam::new(types::I64));
    op_concat_sig.params.push(AbiParam::new(types::I64));
    op_concat_sig.returns.push(AbiParam::new(types::I64));
    let op_concat_id = module
        .declare_function("luna_jit_op_concat", Linkage::Import, &op_concat_sig)
        .ok()?;

    Some(OpHelpers {
        new_table_id,
        set_ids,
        get_field_id,
        get_tab_up_id,
        get_field_checked_id,
        get_tab_up_checked_id,
        op_closure_id,
        spill_id,
        op_close_id,
        op_tforcall_id,
        stack_load_id,
        stack_tag_id,
        op_concat_id,
    })
}

/// An imported helper taking `n_params` `i64`s and returning one.
fn declare_i64_import<M: Module>(module: &mut M, name: &str, n_params: usize) -> Option<FuncId> {
    let mut sig = module.make_signature();
    for _ in 0..n_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    module.declare_function(name, Linkage::Import, &sig).ok()
}

fn declare_runtime_helpers<M: Module>(module: &mut M) -> Option<RuntimeHelpers> {
    // `fn luna_jit_str_buf_acquire() -> i64`.
    // Returns a `*mut Vec<u8>` (boxed-leaked); used by buffered
    // accumulator emit at trace fn entry.
    let mut str_buf_acquire_sig = module.make_signature();
    str_buf_acquire_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_acquire_id = module
        .declare_function(
            "luna_jit_str_buf_acquire",
            Linkage::Import,
            &str_buf_acquire_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_release(buf: i64)`.
    let mut str_buf_release_sig = module.make_signature();
    str_buf_release_sig.params.push(AbiParam::new(types::I64));
    let str_buf_release_id = module
        .declare_function(
            "luna_jit_str_buf_release",
            Linkage::Import,
            &str_buf_release_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_extend(buf, str_ptr) -> i64`.
    let mut str_buf_extend_sig = module.make_signature();
    str_buf_extend_sig.params.push(AbiParam::new(types::I64));
    str_buf_extend_sig.params.push(AbiParam::new(types::I64));
    str_buf_extend_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_extend_id = module
        .declare_function(
            "luna_jit_str_buf_extend",
            Linkage::Import,
            &str_buf_extend_sig,
        )
        .ok()?;

    // `fn luna_jit_str_buf_intern(buf) -> i64`.
    let mut str_buf_intern_sig = module.make_signature();
    str_buf_intern_sig.params.push(AbiParam::new(types::I64));
    str_buf_intern_sig.returns.push(AbiParam::new(types::I64));
    let str_buf_intern_id = module
        .declare_function(
            "luna_jit_str_buf_intern",
            Linkage::Import,
            &str_buf_intern_sig,
        )
        .ok()?;
    // Squelch unused warnings.
    let _ = (
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
    );

    // `fn luna_jit_stack_update_raw(slot, raw)`.
    // Used in Op::Concat operand spill for Unset-kind slots.
    let mut update_raw_sig = module.make_signature();
    update_raw_sig.params.push(AbiParam::new(types::I64));
    update_raw_sig.params.push(AbiParam::new(types::I64));
    let update_raw_id = module
        .declare_function(
            "luna_jit_stack_update_raw",
            Linkage::Import,
            &update_raw_sig,
        )
        .ok()?;

    let mut get_int_sig = module.make_signature();
    get_int_sig.params.push(AbiParam::new(types::I64));
    get_int_sig.params.push(AbiParam::new(types::I64));
    get_int_sig.returns.push(AbiParam::new(types::I64));
    let get_int_id = module
        .declare_function("luna_jit_table_get_int", Linkage::Import, &get_int_sig)
        .ok()?;

    let suppress_admit_id = module
        .declare_function(
            "luna_jit_suppress_trace_admit",
            Linkage::Import,
            &module.make_signature(),
        )
        .ok()?;
    let mut math_fn_check_sig = module.make_signature();
    math_fn_check_sig.params.push(AbiParam::new(types::I64));
    math_fn_check_sig.params.push(AbiParam::new(types::I64));
    math_fn_check_sig.returns.push(AbiParam::new(types::I64));
    let math_fn_check_id = module
        .declare_function(
            "luna_jit_math_fn_is_library",
            Linkage::Import,
            &math_fn_check_sig,
        )
        .ok()?;

    let mut str_sub_sig = module.make_signature();
    for _ in 0..3 {
        str_sub_sig.params.push(AbiParam::new(types::I64));
    }
    str_sub_sig.returns.push(AbiParam::new(types::I64));
    let str_sub_id = module
        .declare_function("luna_jit_str_sub", Linkage::Import, &str_sub_sig)
        .ok()?;

    let mut len_sig = module.make_signature();
    len_sig.params.push(AbiParam::new(types::I64));
    len_sig.returns.push(AbiParam::new(types::I64));
    let len_checked_id = module
        .declare_function("luna_jit_table_len_checked", Linkage::Import, &len_sig)
        .ok()?;

    // `fn luna_jit_upval_get(idx: i64) -> i64`. The
    // helper reads `JIT_CL`'s upvals[idx], unpacks to raw payload,
    // returns it as i64. Type tag is lost across the ABI; the
    // dispatcher's exit_tags must use the Untouched fallback
    // (carry the entry tag through) since we can't statically
    // determine what kind of Value an upval holds.
    let mut upval_get_sig = module.make_signature();
    upval_get_sig.params.push(AbiParam::new(types::I64));
    upval_get_sig.returns.push(AbiParam::new(types::I64));
    let upval_get_checked_id = declare_i64_import(module, "luna_jit_upval_get_checked", 3)?;
    let upval_get_id = module
        .declare_function("luna_jit_upval_get", Linkage::Import, &upval_get_sig)
        .ok()?;
    let mut head_closure_sig = module.make_signature();
    head_closure_sig.returns.push(AbiParam::new(types::I64));
    let head_closure_id = module
        .declare_function("luna_jit_head_closure", Linkage::Import, &head_closure_sig)
        .ok()?;

    // `fn luna_jit_trace_materialize_frames(n: u64,
    // metas: *const FrameMaterializeInfo) -> i64`. Called by the
    // lowerer's cmp@d>0 emit.
    let mut materialize_sig = module.make_signature();
    materialize_sig.params.push(AbiParam::new(types::I64));
    materialize_sig.params.push(AbiParam::new(types::I64));
    materialize_sig.returns.push(AbiParam::new(types::I64));
    let materialize_id = module
        .declare_function(
            "luna_jit_trace_materialize_frames",
            Linkage::Import,
            &materialize_sig,
        )
        .ok()?;

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
    let mut mat_sunk_sig = module.make_signature();
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.params.push(AbiParam::new(types::I64));
    mat_sunk_sig.returns.push(AbiParam::new(types::I64));
    let mat_sunk_id = module
        .declare_function(
            "luna_jit_materialize_sunk_table",
            Linkage::Import,
            &mat_sunk_sig,
        )
        .ok()?;

    Some(RuntimeHelpers {
        str_buf_acquire_id,
        str_buf_release_id,
        str_buf_extend_id,
        str_buf_intern_id,
        update_raw_id,
        get_int_id,
        suppress_admit_id,
        math_fn_check_id,
        str_sub_id,
        len_checked_id,
        upval_get_id,
        upval_get_checked_id,
        head_closure_id,
        materialize_id,
        mat_sunk_id,
    })
}
