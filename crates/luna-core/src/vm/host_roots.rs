//! v1.3 Phase SR — host root pool types and slot-recycling API.
//!
//! This module owns the type definitions ([`HostRootSlot`],
//! [`HostRootTicket`], [`HostRootStale`]) plus the `pin_host` /
//! `read_host` / `write_host` / `unpin` / `unpin_all` /
//! `host_root_count` impls. The `Vm` struct itself (in
//! `crate::vm::exec`) carries two fields:
//!
//! ```ignore
//! pub(crate) host_roots: Vec<HostRootSlot>,
//! pub(crate) host_roots_free: Vec<u32>,
//! ```
//!
//! The pool replaces the v1.1 append-only `Vec<Value>`. Long-running
//! embedders (request-per-script loops, edge workers) now release
//! single pins via [`Vm::unpin`] without forcing `unpin_all` between
//! requests; slots are recycled via a free list, and `HostRootTicket`
//! carries an ABA-safe generation counter so a stale ticket (held
//! across an unpin/re-pin cycle on the same slot) reads as `None` /
//! `Err(HostRootStale)`.
//!
//! The GC tracer (in `crate::vm::exec`) walks each slot's `value`;
//! free-list slots carry `Value::Nil` which is a GC no-op, so we don't
//! bother branching on free vs live in the tracer hot path.
//!
//! See `.dev/rfcs/v1.3-audit-slot-recycling.md` for the design rationale.

use crate::runtime::value::Value;
use crate::vm::exec::Vm;

/// v1.3 Phase SR — one slot in the host root pool.
///
/// `value == Value::Nil` when the slot is on the free list; the GC
/// tracer treats `Nil` as a no-op so free slots cost nothing to
/// trace. `generation` is bumped on every fresh `pin_host`
/// allocation into this slot AND on every `unpin` / `unpin_all`.
#[derive(Copy, Clone, Debug)]
pub(crate) struct HostRootSlot {
    pub(crate) value: Value,
    pub(crate) generation: u32,
}

/// v1.3 Phase SR — opaque handle to a pinned host root.
///
/// `Copy` so embedder handle types (`LuaFunction` / `LuaTable` /
/// `LuaRoot`) stay `Copy`. Two `u32` fields → 8 bytes total, fits in
/// a register; matches `usize` payload size on 64-bit and is smaller
/// on 32-bit.
///
/// Embedders store / copy / compare tickets but cannot mint a fake
/// one — fields are crate-private, the only constructor is
/// [`Vm::pin_host`].
///
/// Generation overflow at `u32::MAX` retires the slot permanently
/// (the index is NOT pushed to the free list; future `pin_host`
/// allocations bypass it). At 10⁹ unpins/day per slot that's ~4 days;
/// lifetime leak is bounded.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct HostRootTicket {
    pub(crate) idx: u32,
    pub(crate) generation: u32,
}

impl HostRootTicket {
    /// Slot index this ticket targets. Diagnostic / facade-author use
    /// only — embedders should not rely on numerical identity.
    pub fn idx(self) -> u32 {
        self.idx
    }

    /// Generation this ticket was issued at. Diagnostic only —
    /// equality against the live slot's current generation determines
    /// validity.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// v1.3 Phase SR — error returned by [`Vm::write_host`] /
/// [`Vm::unpin`] when the supplied ticket's `generation` no longer
/// matches the live slot. Indicates the slot has been unpinned (and
/// possibly re-pinned to an unrelated value) since the ticket was
/// issued.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct HostRootStale;

impl std::fmt::Display for HostRootStale {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("host root ticket is stale (slot was unpinned and possibly re-pinned)")
    }
}

impl std::error::Error for HostRootStale {}

impl Vm {
    /// Pin `v` as a host root. Reuses a recycled slot if the free
    /// list is non-empty, else extends the pool. Bumps the slot's
    /// generation; previously-issued tickets for that slot become
    /// stale (`read_host` returns `None`, `write_host` / `unpin`
    /// return `Err(HostRootStale)`).
    ///
    /// Returns a [`HostRootTicket`] (`Copy`, 8 bytes). The value
    /// becomes an extra GC root until the ticket is released via
    /// [`Self::unpin`] or the whole pool via [`Self::unpin_all`].
    pub fn pin_host(&mut self, v: Value) -> HostRootTicket {
        if let Some(idx) = self.host_roots_free.pop() {
            let slot = &mut self.host_roots[idx as usize];
            // Bump generation on every fresh allocation into this
            // slot so any stale ticket for this index reads as None.
            // Saturating: a retired slot (generation == u32::MAX)
            // would stay retired — its index never appears on the
            // free list, so this branch is normally unreachable for
            // retired slots; the saturating add is defensive.
            slot.generation = slot.generation.saturating_add(1);
            slot.value = v;
            HostRootTicket {
                idx,
                generation: slot.generation,
            }
        } else {
            let idx = self.host_roots.len() as u32;
            // Generation starts at 0 for a freshly allocated slot;
            // `unpin` bumps to 1 before pushing to the free list,
            // and the next `pin_host` into that slot bumps to 2.
            self.host_roots.push(HostRootSlot {
                value: v,
                generation: 0,
            });
            HostRootTicket { idx, generation: 0 }
        }
    }

    /// Read a previously pinned host root. Returns `None` if the
    /// ticket is stale (slot was unpinned and possibly re-pinned to a
    /// different value) or if the ticket index is out of bounds.
    pub fn read_host(&self, t: HostRootTicket) -> Option<Value> {
        let slot = self.host_roots.get(t.idx as usize)?;
        if slot.generation == t.generation {
            Some(slot.value)
        } else {
            None
        }
    }

    /// Mutate a previously pinned host root in place. Returns
    /// `Err(HostRootStale)` on stale ticket; otherwise updates the
    /// slot's value WITHOUT bumping generation (mutation does not
    /// invalidate other live aliases of the same ticket).
    pub fn write_host(&mut self, t: HostRootTicket, v: Value) -> Result<(), HostRootStale> {
        let slot = self
            .host_roots
            .get_mut(t.idx as usize)
            .ok_or(HostRootStale)?;
        if slot.generation == t.generation {
            slot.value = v;
            Ok(())
        } else {
            Err(HostRootStale)
        }
    }

    /// Drop a single pinned root. Clears the slot's value to `Nil`,
    /// bumps the slot's generation, and pushes the index onto the
    /// free list for reuse. Returns `Err(HostRootStale)` if the
    /// ticket is stale (already-unpinned / re-pinned slot); the
    /// pool is unchanged in that case.
    ///
    /// Generation overflow at `u32::MAX` retires the slot
    /// permanently — the index is NOT pushed to the free list, and
    /// future `pin_host` calls will allocate a fresh slot rather
    /// than reuse this one.
    pub fn unpin(&mut self, t: HostRootTicket) -> Result<(), HostRootStale> {
        let slot = self
            .host_roots
            .get_mut(t.idx as usize)
            .ok_or(HostRootStale)?;
        if slot.generation != t.generation {
            return Err(HostRootStale);
        }
        slot.value = Value::Nil;
        if slot.generation == u32::MAX {
            // Retire: do not push to free list. The slot stays at
            // generation == u32::MAX with value Nil; the GC tracer
            // will continue to walk it as a no-op, but no further
            // `pin_host` will reuse it.
            return Ok(());
        }
        slot.generation += 1;
        self.host_roots_free.push(t.idx);
        Ok(())
    }

    /// Number of currently-pinned (live) host roots. Diagnostic only.
    ///
    /// Computed as `host_roots.len() - host_roots_free.len()` — this
    /// over-counts retired-by-overflow slots as still-allocated. For
    /// a short-lived process the difference is sub-MB; long-running
    /// servers should treat this as an upper bound on live pins.
    pub fn host_root_count(&self) -> usize {
        self.host_roots.len() - self.host_roots_free.len()
    }

    /// Drop every pinned host root. Embedders driving the `Lua`
    /// facade in a request-per-script loop call this to release a
    /// batch of pins in one shot. Bumps every slot's generation;
    /// every previously-issued ticket becomes stale uniformly.
    ///
    /// Keeps the underlying `Vec` capacity to amortize future
    /// `pin_host` allocations. Slots that already reached
    /// `generation == u32::MAX` stay retired (not added back to the
    /// free list).
    pub fn unpin_all(&mut self) {
        self.host_roots_free.clear();
        for (i, slot) in self.host_roots.iter_mut().enumerate() {
            slot.value = Value::Nil;
            if slot.generation == u32::MAX {
                // Already retired — skip.
                continue;
            }
            slot.generation += 1;
            self.host_roots_free.push(i as u32);
        }
    }
}
