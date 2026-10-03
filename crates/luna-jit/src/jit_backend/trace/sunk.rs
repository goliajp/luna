use super::*;

/// At an exit emit point (a cmp side exit or a failed guard),
/// materialise every live Sinkable site (depth 0 and inlined frames)
/// into a heap table: stack buffers for the array part (`cap` payloads
/// and tags) and the hash part (keys, payloads, tags) are filled from
/// the site's virtual slots and handed to the materialise helper. The
/// table is written into every register bound to the site at this op,
/// and `kinds_snapshot` marks those registers `RegKind::Table` so the
/// dispatcher's restore repacks them.
///
/// `kinds_snapshot` is either `max_stack`-sized (depth-0 exit; the
/// bounds check skips registers of inlined frames) or window-sized
/// (an exit inside an inlined frame).
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
    let empty: &[LiveBinding] = &[];
    let live: &[LiveBinding] = escape
        .live_at_op
        .get(cmp_op_idx)
        .map(|v| v.as_slice())
        .unwrap_or(empty);
    let mut done: Vec<u32> = Vec::new();
    for b in live {
        let sid = b.site as usize;
        if done.contains(&b.site) || sid >= escape.sites.len() {
            continue;
        }
        done.push(b.site);
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
        // Every register holding the table at this op, in the trace's
        // register window: a `Move` copies only the stale bits of a sunk
        // table, so each copy needs the materialised one. The bindings
        // of a site all sit in its own frame (offset of its NewTable).
        // Registers past `kinds_snapshot` belong to an inlined frame the
        // depth-0 caller's snapshot does not cover.
        let off = op_offsets[site.op_idx] as usize;
        let regs: Vec<usize> = live
            .iter()
            .filter(|l| l.site == b.site)
            .map(|l| off + l.reg as usize)
            .filter(|&r| r < regs_full.len() && r < kinds_snapshot.len())
            .collect();
        if regs.is_empty() {
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
        for r in regs {
            bcx.def_var(regs_full[r], table_bits);
            kinds_snapshot[r] = RegKind::Table;
        }
        count += 1;
    }
    count
}
