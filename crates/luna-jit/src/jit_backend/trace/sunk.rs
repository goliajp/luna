use super::*;

/// at a cmp side-exit emit point, materialise every
/// live Sinkable site at `inline_depth = 0`. For each site:
/// stack-allocate two parallel buffers (`cap × i64` raws + `cap × u8`
/// kind tags), fill from virt slot Variables + `virt_kinds`, call
/// the materialise helper, and `def_var` the returned heap table
/// bits into `regs_full[site.a]` so the subsequent `store_back`
/// lands the heap pointer in `reg_state[site.a]`. Returns the
/// per-exit-tags snapshot (with materialised slots overridden to
/// `RegKind::Table`) plus the number of sites materialised at this
/// emit point.
///
/// `inline_depth > 0` sites are demoted in pre-emit (the `has_inline_cmp`
/// gate), so this function only walks depth-0 sites. Inline sinking
/// is a follow-up (would extend `regs_full[off + site.a]` indexing
/// for the inlined frame's window).
/// at a cmp side-exit emit point,
/// materialise every live Sinkable site (depth=0 AND depth>0) into
/// the heap and update `kinds_snapshot` so the dispatcher's restore
/// path repacks `RegKind::Table` for each materialised slot.
///
/// `kinds_snapshot` is updated in-place for each materialised
/// site's slot — caller passes either a `max_stack`-sized snapshot
/// (depth=0 cmp arm; bounds check skips depth>0 sites by index)
/// or a window-sized snapshot (depth>0 cmp arm; depth>0 sites land
/// inside the window).
///
/// Returns the number of sites materialised at this emit point.
pub(super) fn emit_materialize_live_sunk<E: Emit>(
    bcx: &mut E,
    mat_sunk_id: cranelift_module::FuncId,
    escape: &EscapeAnalysis,
    virt_vars: &[Option<Vec<Variable>>],
    virt_kinds: &[Option<Vec<RegKind>>],
    regs_full: &[Variable],
    op_offsets: &[u32],
    cmp_op_idx: usize,
    kinds_snapshot: &mut [RegKind],
    head_proto: Gc<Proto>,
    // relocation context. `aot
    // == false` keeps the original JIT-time iconst behaviour;
    // `aot == true` routes interned-key pointers through the
    // `__luna_aot_strkey_slot_<hex>` data section. `defined_aot_data`
    // is per-lower-call dedup memory for `define_data` (only the
    // first occurrence of a given DataId may define its bytes).
    aot: bool,
    defined_aot_data: &mut std::collections::HashSet<DataId>,
) -> u32 {
    let mut count: u32 = 0;
    let empty: &[u32] = &[];
    let live: &[u32] = escape
        .live_at_op
        .get(cmp_op_idx)
        .map(|v| v.as_slice())
        .unwrap_or(empty);
    for &sid32 in live {
        let sid = sid32 as usize;
        if sid >= escape.sites.len() {
            continue;
        }
        let site = &escape.sites[sid];
        if site.state != EscapeState::Sinkable {
            continue;
        }
        let Some(vars) = virt_vars[sid].as_ref() else {
            continue;
        };
        let Some(kinds) = virt_kinds[sid].as_ref() else {
            continue;
        };
        let cap = site.array_cap as usize;
        if cap == 0 {
            continue;
        }
        // Site address in the trace's register window: caller-frame
        // address = site.a; inline-frame address = op_offsets[site.op_idx] + site.a.
        let off = op_offsets[site.op_idx] as usize;
        let reg_idx = off + site.a as usize;
        if reg_idx >= regs_full.len() {
            continue;
        }
        // Skip depth>0 sites when caller's snapshot is caller-window
        // only (max_stack sized) — the kind plumbing for those is
        // out of scope for the depth=0 cmp arm.
        if reg_idx >= kinds_snapshot.len() {
            continue;
        }
        let n_hash = site.hash_keys.len();
        // Array stack-alloc buffers; for cap=0 (hash-only site)
        // pass null pointers.
        let (raws_addr, kinds_addr) = if cap > 0 {
            let raws_ss = bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                (cap * 8) as u32,
                3,
            ));
            let kinds_ss = bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                cap as u32,
                0,
            ));
            for vi in 0..cap {
                let v = bcx.use_var(vars[vi]);
                bcx.ins()
                    .stack_store(types::I64, v, raws_ss, (vi * 8) as i32);
                let tag = kind_to_raw_tag(kinds[vi]);
                let k = bcx.ins().iconst(types::I8, tag as i64);
                bcx.ins().stack_store(types::I64, k, kinds_ss, vi as i32);
            }
            (
                bcx.ins().stack_addr(types::I64, raws_ss, 0),
                bcx.ins().stack_addr(types::I64, kinds_ss, 0),
            )
        } else {
            (
                bcx.ins().iconst(types::I64, 0),
                bcx.ins().iconst(types::I64, 0),
            )
        };
        // hash slot stack-alloc buffers (3 parallel
        // arrays: keys, raws, kinds). For each hash slot, fill from
        // virt_vars[cap + slot] + virt_kinds[cap + slot]; key ptr
        // comes from head_proto.consts[site.hash_keys[slot]] at
        // compile time (baked in as iconst).
        let (hash_keys_addr, hash_raws_addr, hash_kinds_addr) = if n_hash > 0 {
            let hash_keys_ss =
                bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    (n_hash * 8) as u32,
                    3,
                ));
            let hash_raws_ss =
                bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    (n_hash * 8) as u32,
                    3,
                ));
            let hash_kinds_ss =
                bcx.create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
                    cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
                    n_hash as u32,
                    0,
                ));
            for vi in 0..n_hash {
                let slot = cap + vi;
                let v = bcx.use_var(vars[slot]);
                bcx.ins()
                    .stack_store(types::I64, v, hash_raws_ss, (vi * 8) as i32);
                let tag = kind_to_raw_tag(kinds[slot]);
                let k = bcx.ins().iconst(types::I8, tag as i64);
                bcx.ins()
                    .stack_store(types::I64, k, hash_kinds_ss, vi as i32);
                let const_idx = site.hash_keys[vi] as usize;
                let key_str = match head_proto.consts[const_idx] {
                    luna_core::runtime::Value::Str(s) => s,
                    _ => unreachable!(
                        "hash_keys must point at Str consts (validated by escape sweep)"
                    ),
                };
                let key_ptr_v = emit_str_key_arg(bcx, key_str, aot, defined_aot_data);
                bcx.ins()
                    .stack_store(types::I64, key_ptr_v, hash_keys_ss, (vi * 8) as i32);
            }
            (
                bcx.ins().stack_addr(types::I64, hash_keys_ss, 0),
                bcx.ins().stack_addr(types::I64, hash_raws_ss, 0),
                bcx.ins().stack_addr(types::I64, hash_kinds_ss, 0),
            )
        } else {
            (
                bcx.ins().iconst(types::I64, 0),
                bcx.ins().iconst(types::I64, 0),
                bcx.ins().iconst(types::I64, 0),
            )
        };
        let cap_val = bcx.ins().iconst(types::I64, cap as i64);
        let n_hash_val = bcx.ins().iconst(types::I64, n_hash as i64);
        let mat_ref = bcx.import_func(mat_sunk_id);
        let call = bcx.ins().call(
            mat_ref,
            &[
                cap_val,
                raws_addr,
                kinds_addr,
                n_hash_val,
                hash_keys_addr,
                hash_raws_addr,
                hash_kinds_addr,
            ],
        );
        let table_bits = bcx.inst_results(call)[0];
        bcx.def_var(regs_full[reg_idx], table_bits);
        kinds_snapshot[reg_idx] = RegKind::Table;
        count += 1;
    }
    count
}
