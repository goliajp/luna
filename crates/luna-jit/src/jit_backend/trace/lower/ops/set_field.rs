//! String-key stores: `SetField`, and a `SetTable` whose key is a string.

use super::*;

/// `Op::SetField`: into a sunk table's slot, or through the checked store helper.
pub(super) fn emit_set_field<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let Plan { opts, .. } = *pl;
    let OpCx { i, off, ins, .. } = *oc;
    let regs: &[Variable] = oc.regs;
    // sunk path: the source register goes into the virtual slot
    // (array_cap + hash_slot) with its kind; PUC's table may grow
    if let Some(OpAction::SetFieldSunkWrite {
        site_idx,
        hash_slot,
        grow_to,
    }) = lw.escape.op_actions[i]
        && lw.escape.sites[site_idx as usize].state == EscapeState::Sinkable
        && lw.virt_vars[site_idx as usize].is_some()
    {
        let array_cap = lw.escape.sites[site_idx as usize].array_cap as usize;
        let slot = array_cap + hash_slot as usize;
        let src_kind = lw.current_kinds[off + ins.c() as usize];
        let v = lw.bcx.use_var(regs[ins.c() as usize]);
        let vars = lw.virt_vars[site_idx as usize]
            .as_ref()
            .expect("Sinkable site has virt_vars");
        lw.bcx.def_var(vars[slot], v);
        let kinds_vec = lw.virt_kinds[site_idx as usize]
            .as_mut()
            .expect("Sinkable site has virt_kinds");
        kinds_vec[slot] = src_kind;
        return emit_sunk_growth(lw, site_idx as usize, grow_to);
    }
    // helper path: R[A][K[B]:string] := R[C].
    // the helpers read the operand as a table. A number,
    // string or closure (entry-guarded or computed here) leaves
    // the op to the interpreter; Nil can be a lookahead guess
    // for a value the recording indexed, so it stays
    match k_op(&lw.current_kinds, off as u32 + ins.a()) {
        RegKind::Table | RegKind::Nil => {}
        RegKind::Unset | RegKind::Unknown => {
            lw.dispatchable = false;
            lw.dispatch_off_reason = lw.dispatch_off_reason.or(Some("table-op:unknown-kind"));
        }
        _ => return None,
    }
    let t = lw.bcx.use_var(regs[ins.a() as usize]);
    let key_v = match oc.rop.proto.consts[ins.b() as usize] {
        luna_core::runtime::Value::Str(s) => s,
        _ => unreachable!("pre-emit gates Str const at K[B]"),
    };
    let key_arg = emit_str_key_arg(&mut lw.bcx, key_v, opts.aot, &mut lw.defined_aot_data);
    let val_kind = k_op(&lw.current_kinds, off as u32 + ins.c());
    // a value of unknown kind cannot be tagged for the table
    if val_kind.untyped() {
        return None;
    }
    let val = lw.bcx.use_var(regs[ins.c() as usize]);
    store_str_key(lw, pl, oc, t, key_arg, val, val_kind, true)
}

/// `t[key] = val` for an interned string `key`: a number overwriting a
/// value already under a `constant` key goes straight into the hash slot
/// the recording found it in; anything else through the checked helper.
#[allow(clippy::too_many_arguments)]
pub(super) fn store_str_key<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    t: Value,
    key_arg: Value,
    val: Value,
    val_kind: RegKind,
    constant: bool,
) -> Option<()> {
    let OpHelpers { set_ids, .. } = lw.h.op;
    let OpCx { i, rop, .. } = *oc;
    let slot = pl
        .record
        .field_slot(i)
        .filter(|_| constant)
        .filter(|_| matches!(val_kind, RegKind::Int | RegKind::Float));
    let test = slot.is_some() && store_tests_readonly(lw, oc.off + oc.ins.a() as usize, t);
    let merge = slot.map(|slot| {
        let bcx = &mut lw.bcx;
        let hit = bcx.create_block();
        bcx.append_block_param(hit, types::I64);
        let miss = bcx.create_block();
        let merge = bcx.create_block();
        if test {
            array_slot::emit_writable_guard(bcx, t, miss);
        }
        field_slot::emit_field_slot_check(bcx, t, key_arg, slot, None, hit, miss);
        bcx.switch_to_block(hit);
        bcx.seal_block(hit);
        let node = bcx.block_params(hit)[0];
        field_slot::emit_slot_store(bcx, node, val, kind_tag(val_kind));
        bcx.ins().jump(merge, &[]);
        bcx.switch_to_block(miss);
        bcx.seal_block(miss);
        merge
    });
    let done = emit_table_set(
        &mut lw.bcx,
        &set_ids,
        t,
        key_arg,
        RegKind::Str,
        val,
        val_kind,
    );
    guard!(lw, pl, done, i, rop.pc);
    if let Some(merge) = merge {
        lw.bcx.ins().jump(merge, &[]);
        lw.bcx.switch_to_block(merge);
        lw.bcx.seal_block(merge);
    }
    Some(())
}
