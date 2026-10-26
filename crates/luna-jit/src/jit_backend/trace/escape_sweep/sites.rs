//! The allocation sites the escape sweep makes and the hash slots their
//! `SetField`s take.

use super::*;

/// `NewTable`: a site for every shape. A hash-only one (b == 0) has no
/// array part and sinks through `SetField` / `GetField` alone; a mixed one
/// sinks its array part, its hash slots gathered by the `SetField` scan.
#[allow(clippy::too_many_arguments)]
pub(super) fn new_table_site(
    i: usize,
    rop: &RecordedOp,
    depth: u8,
    ins: Inst,
    a: u32,
    bindings: &mut [Vec<Option<usize>>],
    sites: &mut Vec<AllocSite>,
    op_actions: &mut [Option<OpAction>],
) {
    // admit all NewTable shapes as
    // potential sites. For hash-only (b == 0) the
    // array_cap is 0 and the site only sunk-emits via
    // SetField/GetField. For mixed (b > 0, c > 0) the
    // array part sunk-emits; hash slots accumulate via
    // SetField scan.
    let sizes = luna_core::__private::new_table_sizes(ins.b(), ins.c(), ins.k());
    unbind(bindings, depth, a);
    let sid = sites.len();
    sites.push(AllocSite {
        op_idx: i,
        pc: rop.pc,
        a,
        inline_depth: depth,
        array_cap: sizes.map_or(0, |(asize, _)| asize as u32),
        hash_cap: sizes.map_or(0, |(_, h)| {
            if h == 0 {
                0
            } else {
                h.next_power_of_two() as u32
            }
        }),
        table_ops: crate::jit_backend::pack_table_ops(ins),
        hash_keys: Vec::new(),
        // sizes past a table's limit: the helper raises the error
        state: if sizes.is_some() {
            EscapeState::Sinkable
        } else {
            EscapeState::Escaped
        },
    });
    bindings[depth as usize][a as usize] = Some(sid);
    // tag this op so emit can take the sunk
    // path (no NewTable helper call).
    op_actions[i] = Some(OpAction::NewTableSite {
        site_idx: sid as u32,
    });
}

/// The hash slot of key `key` in site `sid`, added when new, and the
/// capacity a new slot past the hash part grows it to (0: none); a site
/// with an array part that must grow escapes.
pub(super) fn hash_slot(sites: &mut [AllocSite], sid: usize, key: u32) -> (usize, u32) {
    let slot_opt = sites[sid].hash_keys.iter().position(|&k| k == key);
    let slot = match slot_opt {
        Some(s) => s,
        None => {
            let s = sites[sid].hash_keys.len();
            sites[sid].hash_keys.push(key);
            s
        }
    };
    let site = &mut sites[sid];
    let grow_to = if slot_opt.is_none() && slot as u32 >= site.hash_cap {
        site.hash_cap = (slot as u32 + 1).next_power_of_two();
        site.hash_cap
    } else {
        0
    };
    if grow_to != 0 && site.array_cap != 0 {
        mark_escape(sites, sid);
    }
    (slot, grow_to)
}
