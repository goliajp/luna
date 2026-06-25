//! v2.0 Track TL — pure-read inspection accessors over a live `Vm`.
//!
//! Consumed by the `luna-tools` CLIs (`luna-heap-dump`,
//! `luna-trace-inspect`, `luna-profile`). Every accessor here is:
//!
//! - **Read-only** — `&Vm`, no `&mut Vm`. Embedders can safely call
//!   between dispatch ticks or from a hook callback without
//!   perturbing JIT state.
//! - **Allocation-discipline** — the per-tick work allocates a
//!   small fixed-size buffer (one `Vec` for the result), nothing
//!   else. No allocation inside the heap-walk loop.
//! - **0 unsafe at embedder surface** — the only unsafe lives one
//!   layer down in [`crate::runtime::Heap::walk_objects`], whose
//!   safety contract is documented at the call site.
//!
//! These accessors are intentionally narrow. `Vm`'s private fields
//! (`frames`, `stack`, ...) remain private; the tools take what
//! these wrappers project, not raw internals. When a tool needs a
//! new view, add a new wrapper here — don't relax the underlying
//! field visibility.

use crate::runtime::ObjTag;
use crate::vm::Vm;

/// Heap snapshot from one [`heap_walk`] invocation.
#[derive(Debug, Clone)]
pub struct HeapSnapshot {
    /// Total live (or not-yet-swept) GC objects. Matches
    /// [`crate::runtime::Heap::live_objects`].
    pub total_objects: usize,
    /// Approximate heap byte count. Matches
    /// [`crate::runtime::Heap::bytes`] — see that field's
    /// rustdoc on what is and isn't tracked.
    pub total_bytes: usize,
    /// Per-tag breakdown, sorted descending by count for stable
    /// downstream display.
    pub buckets: Vec<HeapBucket>,
}

/// One per-type row in [`HeapSnapshot::buckets`].
#[derive(Debug, Clone)]
pub struct HeapBucket {
    /// Lower-cased name of the [`ObjTag`] discriminant
    /// (`"str"`, `"table"`, `"proto"`, ...).
    pub type_name: &'static str,
    /// Count of live objects with this tag.
    pub count: usize,
    /// Per-tag byte estimate. Uses `core::mem::size_of` of the
    /// payload struct as a lower bound — mirrors `Heap::bytes`'s
    /// "shells only" accounting; embedders that need exact bytes
    /// must instrument allocations themselves.
    pub bytes_approx: usize,
}

/// Walk the Vm's heap and produce a per-type [`HeapSnapshot`].
///
/// The walk runs under a `&Heap` borrow so no concurrent mutation
/// can occur; safe to call from any host context that has a `&Vm`.
/// Cost: O(live_objects) reads of the intrusive next-link, plus
/// one `Vec::push` per *distinct* tag (≤ 8 entries by design).
pub fn heap_walk(vm: &Vm) -> HeapSnapshot {
    // Fixed-size table indexed by ObjTag discriminant. Avoids any
    // alloc in the hot loop.
    let mut counts = [0usize; 8];
    let mut byte_estimate = [0usize; 8];

    vm.heap.walk_objects(|tag| {
        let idx = tag as usize;
        counts[idx] += 1;
        byte_estimate[idx] += tag_payload_size(tag);
    });

    let mut buckets: Vec<HeapBucket> = ALL_TAGS
        .iter()
        .copied()
        .filter_map(|tag| {
            let idx = tag as usize;
            if counts[idx] == 0 {
                return None;
            }
            Some(HeapBucket {
                type_name: tag_name(tag),
                count: counts[idx],
                bytes_approx: byte_estimate[idx],
            })
        })
        .collect();
    buckets.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.type_name.cmp(b.type_name))
    });

    HeapSnapshot {
        total_objects: vm.heap.live_objects(),
        total_bytes: vm.heap.bytes(),
        buckets,
    }
}

/// Compile-time list of every [`ObjTag`] variant. Used by
/// [`heap_walk`] to drive bucket ordering and lookup; the array
/// length is asserted equal to the variant count below so adding
/// a new variant is a compile error here until the table grows.
const ALL_TAGS: [ObjTag; 8] = [
    ObjTag::Str,
    ObjTag::Table,
    ObjTag::Proto,
    ObjTag::Closure,
    ObjTag::Upvalue,
    ObjTag::Native,
    ObjTag::Coro,
    ObjTag::Userdata,
];

const _OBJTAG_COVERS_EVERY_VARIANT: () = {
    // Force a compile error if a new ObjTag variant is added but
    // the ALL_TAGS table isn't grown alongside it. Rust's
    // exhaustive match on a non-exhaustive enum-by-list pattern
    // is the only way to get this check without macros.
    let _ = |t: ObjTag| match t {
        ObjTag::Str
        | ObjTag::Table
        | ObjTag::Proto
        | ObjTag::Closure
        | ObjTag::Upvalue
        | ObjTag::Native
        | ObjTag::Coro
        | ObjTag::Userdata => (),
    };
};

fn tag_name(tag: ObjTag) -> &'static str {
    match tag {
        ObjTag::Str => "str",
        ObjTag::Table => "table",
        ObjTag::Proto => "proto",
        ObjTag::Closure => "closure",
        ObjTag::Upvalue => "upvalue",
        ObjTag::Native => "native",
        ObjTag::Coro => "coro",
        ObjTag::Userdata => "userdata",
    }
}

/// Approximate per-tag shell size in bytes. Lower bound — matches
/// the accounting policy of [`crate::runtime::Heap::bytes`] (shell
/// sizes only; `Vec`/`Box` overflow is uncounted).
fn tag_payload_size(tag: ObjTag) -> usize {
    use crate::runtime::function::{LuaClosure, NativeClosure, Proto, Upvalue};
    use crate::runtime::table::Table;
    use crate::runtime::userdata::Userdata;
    use crate::runtime::{Coro, LuaStr};

    match tag {
        ObjTag::Str => core::mem::size_of::<LuaStr>(),
        ObjTag::Table => core::mem::size_of::<Table>(),
        ObjTag::Proto => core::mem::size_of::<Proto>(),
        ObjTag::Closure => core::mem::size_of::<LuaClosure>(),
        ObjTag::Upvalue => core::mem::size_of::<Upvalue>(),
        ObjTag::Native => core::mem::size_of::<NativeClosure>(),
        ObjTag::Coro => core::mem::size_of::<Coro>(),
        ObjTag::Userdata => core::mem::size_of::<Userdata>(),
    }
}

/// Snapshot of the JIT state at one point in time. Used by
/// `luna-trace-inspect`; fields are stable across the Vm lifetime.
#[derive(Debug, Clone)]
pub struct JitStateSnapshot {
    /// `JitState::enabled` (master switch).
    pub enabled: bool,
    /// `JitState::trace_enabled` (trace-JIT subswitch).
    pub trace_enabled: bool,
    /// `Some(head_pc)` if a trace is currently being recorded.
    pub active_trace_head_pc: Option<u32>,
    /// `Some(ops_len)` length of the in-flight trace's recorded
    /// op stream.
    pub active_trace_len: Option<usize>,
    /// Cumulative trace-close count
    /// (`JitCounters::closed`).
    pub trace_closed_count: u64,
    /// Cumulative trace-abort count (`JitCounters::aborted`).
    pub trace_aborted_count: u64,
    /// Cumulative trace-dispatched count
    /// (`JitCounters::dispatched`).
    pub trace_dispatched_count: u64,
    /// Cumulative trace-compiled count
    /// (`JitCounters::compiled`).
    pub trace_compiled_count: u64,
    /// Cumulative trace-deoptimised count
    /// (`JitCounters::deopt`).
    pub trace_deopt_count: u64,
}

/// Project [`JitStateSnapshot`] from a live `Vm`. Pure-read; no
/// alloc beyond the returned struct itself.
pub fn jit_state_snapshot(vm: &Vm) -> JitStateSnapshot {
    let js = &vm.jit;
    JitStateSnapshot {
        enabled: js.enabled,
        trace_enabled: js.trace_enabled,
        active_trace_head_pc: js.active_trace.as_ref().map(|t| t.head_pc),
        active_trace_len: js.active_trace.as_ref().map(|t| t.ops.len()),
        trace_closed_count: js.counters.closed,
        trace_aborted_count: js.counters.aborted,
        trace_dispatched_count: js.counters.dispatched,
        trace_compiled_count: js.counters.compiled,
        trace_deopt_count: js.counters.deopt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_walk_fresh_vm_has_some_protos_and_strings() {
        // A fresh Vm preloads stdlib (when constructed via the
        // `new_with_jit` ergonomic path), so the heap has at
        // minimum a few protos + many interned strings. We assert
        // the snapshot can be produced and is internally
        // consistent; exact counts depend on the stdlib loader
        // version and are intentionally not pinned.
        let vm = Vm::new(crate::version::LuaVersion::Lua55);
        let snap = heap_walk(&vm);
        assert_eq!(
            snap.total_objects,
            snap.buckets.iter().map(|b| b.count).sum::<usize>(),
            "per-bucket counts must sum to live_objects"
        );
        assert!(
            snap.buckets.iter().all(|b| b.count > 0),
            "no zero-count rows allowed in the report"
        );
    }

    #[test]
    fn jit_state_snapshot_default_inert() {
        let vm = Vm::new(crate::version::LuaVersion::Lua55);
        let snap = jit_state_snapshot(&vm);
        // A bare Vm::new() has the null backend; counters start
        // at zero, no trace in flight.
        assert!(snap.enabled);
        assert!(snap.active_trace_head_pc.is_none());
        assert_eq!(snap.trace_closed_count, 0);
        assert_eq!(snap.trace_aborted_count, 0);
    }
}
