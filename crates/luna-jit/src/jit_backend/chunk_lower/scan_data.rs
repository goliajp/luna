//! The scan of data-moving and arithmetic ops.

use super::*;

pub(super) fn scan_data(s: &mut ChunkScan, c: ChunkIn<'_>, ins: Inst) -> Option<()> {
    let ChunkIn { proto, .. } = c;
    let ChunkScan {
        self_upval,
        step_const,
        defines_table,
        ..
    } = s;
    match ins.op() {
        Op::LoadI => {
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = Some(ins.sbx() as i64);
            }
        }
        Op::LoadF => {
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        Op::LoadK => {
            // Float constants pass. Int constants also
            // pass. Lua compilers reach for `LoadK Int(v)` when the
            // immediate doesn't fit in `LoadI`'s ±MAX_SBX range
            // (e.g. `for i = 1, 1000000` puts 1000000 in a
            // constant slot). String / Bool / Nil LoadK still bails.
            let bx = ins.bx() as usize;
            let k = proto.consts.get(bx).copied();
            if !matches!(k, Some(LuaValue::Float(_)) | Some(LuaValue::Int(_))) {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                // a `LoadK Int(v)` also pins the register
                // to a known compile-time constant. ForPrep can
                // use this register as its step source just like
                // a `LoadI`.
                *slot = match k {
                    Some(LuaValue::Int(v)) => Some(v),
                    _ => None,
                };
            }
        }
        Op::LoadNil => {
            // `R[A..=A+B] = nil`. The whitelist accepts
            // LoadNil for the cross_dialect `binary_trees` shape
            // (`{nil, nil}` leaf), where the freshly-NewTable'd
            // array slots are written nil by LoadNil and then
            // SetList-stored. Every Lua writer that LoadNil
            // overrides clears the per-reg trackers; downstream
            // SetList emit detects the Nil writer via the BB-local
            // `current_is_nil` shadow and tags `RAW_TAG_NIL` instead
            // of the default Int tag. Arith / cmp on a Nil-written
            // register would silently treat 0 as an Int value;
            // bail those readers below (in the kind sweep and the
            // arith linear-pass) instead of risking miscompile.
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            for off in 0..=b {
                let r = a + off;
                if let Some(slot) = self_upval.get_mut(r) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(r) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(r) {
                    *slot = false;
                }
            }
        }
        Op::Move => {
            let a = ins.a() as usize;
            let b = ins.b() as usize;
            let tag = self_upval.get(b).copied().unwrap_or(false);
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = tag;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
            // propagate table-defined-ness through
            // Move. Note this is a single-pass walk; the
            // fixed-point below catches cases where the Move
            // precedes the NewTable in source order (back-edge
            // through a loop).
            let src_def = defines_table.get(b).copied().unwrap_or(false);
            if let Some(slot) = defines_table.get_mut(a) {
                *slot = src_def;
            }
        }
        Op::Add | Op::Sub | Op::Mul | Op::Div => {
            // Reading a self-upval-tagged register in arith means the
            // GetUpval was a generic upvalue read (e.g., `n + 1` over
            // an outer-local upvalue), not the self-recursion shortcut.
            // Bail out — only the call-target case is handled.
            let b = ins.b() as usize;
            let c = ins.c() as usize;
            if self_upval.get(b).copied().unwrap_or(false)
                || self_upval.get(c).copied().unwrap_or(false)
            {
                return None;
            }
            let a = ins.a() as usize;
            if let Some(slot) = self_upval.get_mut(a) {
                *slot = false;
            }
            if let Some(slot) = step_const.get_mut(a) {
                *slot = None;
            }
        }
        _ => unreachable!("dispatched by op"),
    }
    Some(())
}
