//! The tables a trace sinks: PUC makes each one, so each is counted and
//! recorded as garbage, unless the trace leaves with it.

use super::*;
use crate::runtime::heap::{GcHeader, GhostRef};
use crate::runtime::table::new_table_sizes;

/// The records of PUC's objects a running trace stands for, while luna
/// holds none for them; a trace that runs inside another (a helper's Lua
/// call) has its own.
#[derive(Default)]
pub(crate) struct TraceGhosts {
    /// by sunk site: the record of the table PUC made for it this round
    pub(crate) sunk: Vec<Option<GhostRef>>,
    /// the string PUC made last for the trace's accumulated
    /// concatenation: the one it entered with, or the record of a long
    /// one, which the buffer's string stands for once made
    pub(crate) accum: Option<crate::vm::exec::AccumLast>,
}

impl Vm {
    /// A trace starts running: its records, apart from those of the
    /// traces it runs inside.
    pub(crate) fn jit_ghosts_enter(&mut self) {
        let d = self.jit.ghost_depth;
        if self.jit.ghosts.len() <= d {
            self.jit.ghosts.push(TraceGhosts::default());
        }
        self.jit.ghosts[d].sunk.fill(None);
        self.jit.ghosts[d].accum = None;
        self.jit.ghost_depth = d + 1;
    }

    /// The running trace's records.
    pub(crate) fn jit_ghosts(&mut self) -> &mut TraceGhosts {
        let d = self.jit.ghost_depth.max(1) - 1;
        if self.jit.ghosts.len() <= d {
            self.jit.ghosts.push(TraceGhosts::default());
        }
        &mut self.jit.ghosts[d]
    }

    /// A trace's sunk site `site` makes its table this round (a `NewTable`
    /// sized by `b`, `c`, `k`): `true` when a collection step falls due
    /// with it, the trace then leaving with the table made for real;
    /// otherwise it is counted and recorded.
    #[doc(hidden)]
    pub fn jit_sunk_table(&mut self, site: usize, b: u32, c: u32, k: bool) -> bool {
        let sunk = &mut self.jit_ghosts().sunk;
        if sunk.len() <= site {
            sunk.resize(site + 1, None);
        }
        sunk[site] = None;
        let Some((a, h)) = new_table_sizes(b, c, k) else {
            return false;
        };
        let h = if h == 0 { 0 } else { h.next_power_of_two() };
        let size = self.heap.puc_table_bytes(a, h);
        if self.heap.gc_due_after(size) {
            self.jit.gc_exit = Some(super::JIT_GC_PREV);
            return true;
        }
        // no memory for the record: the table is not counted, as when the
        // trace leaves with it before it is
        let g = self.heap.acct_ghost_table(a, h).ok();
        self.jit_ghosts().sunk[site] = g;
        false
    }

    /// Sunk site `site`'s table grows from `old_h` nodes to `new_h` (a
    /// field it had no room for; the site has no array part).
    #[doc(hidden)]
    pub fn jit_sunk_grow(&mut self, site: usize, old_h: usize, new_h: usize) {
        if let Some(Some(t)) = self.jit_ghosts().sunk.get(site).copied() {
            self.heap.ghost_table_resize(t, 0, old_h, 0, new_h);
        }
    }

    /// A trace leaves with sunk site `site`'s table, which it makes for
    /// real now: when PUC's was counted this round, it is made without
    /// counting until [`Vm::jit_sunk_made`]. Returns whether it was.
    #[doc(hidden)]
    pub fn jit_sunk_take(&mut self, site: usize) -> bool {
        let counted = self
            .jit_ghosts()
            .sunk
            .get(site)
            .is_some_and(Option::is_some);
        if counted {
            self.heap.puc.silent += 1;
        }
        counted
    }

    /// The table [`Vm::jit_sunk_take`] was for is made (`t`; `None` when it
    /// could not be): when PUC's was counted, it stands for that one.
    #[doc(hidden)]
    pub fn jit_sunk_made(&mut self, site: usize, counted: bool, t: Option<Gc<Table>>) {
        if !counted {
            return;
        }
        self.heap.puc.silent -= 1;
        let g = self.jit_ghosts().sunk[site].take();
        if let (Some(g), Some(t)) = (g, t) {
            self.heap.ghost_table_taken(g);
            self.heap.set_counted(t.as_ptr() as *mut GcHeader);
        }
    }

    /// A collection starts, perhaps inside a running trace (a Lua function
    /// it calls collects): the sweep may free the records its sunk tables
    /// and its accumulated string are kept in, so the trace lets go of
    /// them, and makes what it leaves with counted.
    pub(crate) fn jit_forget_ghosts(&mut self) {
        for g in &mut self.jit.ghosts {
            g.sunk.fill(None);
            g.accum = None;
        }
    }
}

impl Vm {
    /// Before a collection's atomic phase: the objects a trace being
    /// recorded went through (a native it called collects), watched
    /// without being kept alive.
    pub(crate) fn watch_recording(&mut self) {
        let w = &mut self.heap.weak_objs;
        w.clear();
        let Some(rec) = self.jit.active_trace.as_deref() else {
            return;
        };
        let h = |p: crate::runtime::Gc<crate::runtime::Proto>| p.as_ptr() as *const GcHeader;
        let protos = std::iter::once(rec.head_proto)
            .chain(rec.ops.iter().map(|o| o.proto))
            .chain(rec.side_trace_parent.map(|p| p.0))
            .chain(rec.retfs.iter().map(|r| r.proto))
            .chain(rec.downrec_close.as_ref().map(|d| d.target_proto));
        let key = rec.index_key.map(|s| s.as_ptr() as *const GcHeader);
        w.extend(protos.map(h).chain(key).map(|o| (o, false)));
    }

    /// After it: a recording that went through an object the collection
    /// found dead, a prototype a load it called freed, is dropped, to be
    /// recorded again from the start.
    pub(crate) fn forget_dead_recording(&mut self) {
        let w = std::mem::take(&mut self.heap.weak_objs);
        if w.iter().any(|o| o.1) && self.jit.active_trace.take().is_some() {
            self.jit.counters.aborted += 1;
            self.jit.counters.bump_close_cause("collection");
        }
    }
}
