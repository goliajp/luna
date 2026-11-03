//! What the recorder captures per op, and the field cache snapshot.

use super::*;

/// A single bytecode op as captured during trace recording, with the
/// runtime context needed to emit cranelift guards (register kinds,
/// metatable null checks, etc.). Stored in `TraceRecord.ops`.
#[derive(Clone, Debug)]
pub struct RecordedOp {
    /// Original Proto + PC that produced this op. Multiple
    /// `RecordedOp`s with different `proto` come from inlined calls.
    pub proto: Gc<Proto>,
    /// Pc within `proto` at which this op was recorded.
    pub pc: u32,
    /// The bytecode instruction itself (copy — Proto.code is
    /// already immutable post-compile).
    pub inst: Inst,
    /// Depth of inlined recursion above the trace head. 0 = the
    /// outer trace; positive values come from inlining.
    pub inline_depth: u8,
    /// Recorder snapshot of the runtime variable count
    /// for ops whose B / C field is `0` (meaning "use stack top").
    /// - `Op::Call` with `C == 0`: snapshot of `top - A` AFTER the
    ///   call returns — i.e. the actual number of values the
    ///   callee returned this trip.
    /// - `Op::SetList` with `B == 0`: snapshot of `top - A` at the
    ///   op — i.e. the number of source slots `[A+1..top]`.
    /// - All other ops: `None`.
    /// Emit consumes this as a compile-time constant guarded by a
    /// runtime equality check.
    pub var_count: Option<u32>,
}

/// `LUNA_JIT_FIELD_IC` env gate.
///
/// Default OFF (env unset or set to anything other than `1` / `true`).
/// When ON, the trace recorder captures a [`FieldIcSnapshot`] at the
/// first eligible `Op::GetField` site and the trace lowerer replaces
/// the helper call at that site with an inline cache: 4 guards
/// (`mt is None`, `nodes.len() == cached`, `nodes[slot].key ==
/// cached_key_bits`, `val.tag == cached_tag`) + 1 load of the value's
/// raw payload. Guard miss falls through to the existing helper
/// (scaffold-safe, no new deopt edges).
///
/// Read once per process and cached. It is only the default of each new
/// Vm's switch (`Vm::set_field_ic_enabled`): the recorder checks the
/// Vm's switch, and the lowerer emits the cache wherever the record
/// carries a snapshot.
pub fn field_ic_enabled() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    // 0 = uninitialised, 1 = off, 2 = on. Sentinel encoding lets a
    // single relaxed load distinguish "decision cached" from "ask
    // env" without a Mutex.
    static CACHED: AtomicU8 = AtomicU8::new(0);
    let v = CACHED.load(Ordering::Relaxed);
    if v != 0 {
        return v == 2;
    }
    let enabled = std::env::var("LUNA_JIT_FIELD_IC")
        .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    CACHED.store(if enabled { 2 } else { 1 }, Ordering::Relaxed);
    enabled
}

/// Table-field IC snapshot captured by the recorder
/// at the **first** eligible `Op::GetField` site in the trace, when the
/// recording Vm has its field IC switch on.
///
/// "Eligible" means the receiver `R[B]` is `Value::Table` with no
/// metatable at recorder-fire time AND the key resolves to a
/// `Value::Str` in `head_proto.consts[C]` AND the key actually
/// occupies a hash slot at recording time. The recorder bakes the
/// cached `(nodes_len, slot_idx, key_ptr_bits, val_tag)` tuple here so
/// the lowerer can emit guards against the table's live layout.
///
/// Only a single snapshot is supported (the first eligible site).
#[derive(Clone, Copy, Debug)]
pub struct FieldIcSnapshot {
    /// Index into `TraceRecord.ops` of the `Op::GetField` this
    /// snapshot describes. The lowerer matches `op_idx == i` at
    /// emit time to decide whether to fire the IC path or fall
    /// through to the original helper-call path.
    pub op_idx: u32,
    /// The table's hash-part node count at recorder-fire time. The IC's
    /// shape guard (load the node mask, compare with this count less one)
    /// bails to the helper on mismatch so a rehash deopts predictably.
    pub nodes_len: u64,
    /// Index of the `Node` slot that holds the cached key. The IC
    /// computes `node_addr = nodes_ptr + slot_idx * SIZEOF_NODE`
    /// and reads `key`/`val` from that address.
    pub slot_idx: u64,
    /// `Gc<LuaStr>` raw pointer bits for the cached key. The IC's
    /// slot-key guard (`load i64, icmp Equal, cached_key_bits`)
    /// catches the case where a rehash or insert relocated a
    /// different key into the cached slot.
    pub key_ptr_bits: u64,
    /// Recorder-time tag byte of the slot's value. The IC's val
    /// guard (`load i8, icmp Equal, cached_val_tag`) catches the
    /// case where a `SetField` mutated the slot to a different
    /// type since recording — without this guard the lowerer's
    /// downstream `current_kinds` propagation could pack stale
    /// raw bits as the wrong tag → garbage Value on the next op.
    pub cached_val_tag: u8,
}
