use super::*;

/// The root list for a helper that can run the collector, called at op `i`
/// with everything from absolute register `live_end` up dead or already on
/// the Lua stack: the address of a stack slot holding `n` and then a tag
/// and payload word per collectable value the trace holds only in a
/// register, or 0 when there is none.
///
/// The trace writes registers back to the stack only when it exits, so
/// below `live_end` a register of a collectable kind may hold a table,
/// string or closure the stack does not (one built by this trace, or read
/// from somewhere the trace since overwrote). Fields of live sunk tables
/// are in the same position. Registers of kinds the lowerer does not know
/// are on the stack: the generic-for call stores its results there, and
/// any other unknown kind takes the trace off dispatch.
pub(super) fn emit_ssa_roots<E: Emit>(lw: &mut Lower<E>, i: usize, live_end: usize) -> Value {
    let mut vals: Vec<(Variable, u8)> = Vec::new();
    for (r, &k) in lw.current_kinds[..live_end].iter().enumerate() {
        if let Some(tag) = gc_tag(k) {
            vals.push((lw.regs_full[r], tag));
        }
    }
    let empty: &[LiveBinding] = &[];
    let live = lw.escape.live_at_op.get(i).map_or(empty, |v| v.as_slice());
    let mut seen: Vec<u32> = Vec::new();
    for b in live {
        if seen.contains(&b.site) {
            continue;
        }
        seen.push(b.site);
        let sid = b.site as usize;
        let sunk = lw.escape.sites.get(sid).map(|s| s.state) == Some(EscapeState::Sinkable);
        if let (true, Some(vars), Some(kinds)) = (
            sunk,
            lw.virt_vars[sid].as_ref(),
            lw.virt_kinds[sid].as_ref(),
        ) {
            for (v, &k) in vars.iter().zip(kinds) {
                if let Some(tag) = gc_tag(k) {
                    vals.push((*v, tag));
                }
            }
        }
    }
    if vals.is_empty() {
        return lw.bcx.ins().iconst(types::I64, 0);
    }
    let words = 1 + 2 * vals.len();
    let ss = lw
        .bcx
        .create_sized_stack_slot(cranelift_codegen::ir::StackSlotData::new(
            cranelift_codegen::ir::StackSlotKind::ExplicitSlot,
            (words * 8) as u32,
            3,
        ));
    let n = lw.bcx.ins().iconst(types::I64, vals.len() as i64);
    lw.bcx.ins().stack_store(types::I64, n, ss, 0);
    for (j, (var, tag)) in vals.into_iter().enumerate() {
        let off = (8 + 16 * j) as i32;
        let t = lw.bcx.ins().iconst(types::I64, i64::from(tag));
        lw.bcx.ins().stack_store(types::I64, t, ss, off);
        let v = lw.bcx.use_var(var);
        lw.bcx.ins().stack_store(types::I64, v, ss, off + 8);
    }
    lw.bcx.ins().stack_addr(types::I64, ss, 0)
}

/// The tag of a register of kind `k` when its value is collectable.
fn gc_tag(k: RegKind) -> Option<u8> {
    known_tag(k).filter(|&t| luna_core::runtime::value::raw::is_gc(t))
}
