//! The tables a trace sinks (makes only in its registers): counted as
//! PUC makes them, grown, and made for real at a side exit.

use crate::current_jit_vm;
use crate::table_new::unpack_table_ops;
use luna_core::__private::{GcInternals, ValueInternals, VmJitAlloc};
use luna_core::__private::{RawVal, TableStores, VmHeap};
use luna_core::runtime::{Gc, LuaStr, Table, Value};

/// A trace's sunk site makes its table this round (`ops` its `NewTable`'s
/// packed operands, the site's number + 1 from bit 32): 1 when a
/// collection step falls due with it, the trace then leaving after the
/// `NewTable` with the table made for real; 0 when it is counted and
/// recorded, as PUC's is.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_sunk_table`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_sunk_table(ops: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let (b, c, k) = unpack_table_ops(ops);
    i64::from(vm.jit_sunk_table(sunk_site(ops), b, c, k))
}

/// A trace's sunk site (`ops` as [`luna_jit_sunk_table`] takes them) puts a
/// new field its table has no room for: PUC's grows to `nodes` nodes, from
/// half as many. Returns 0.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread.
// SAFETY: no other item in the link is named `luna_jit_sunk_grow`: only this crate defines
// `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_sunk_grow(ops: i64, nodes: i64) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to this call
    let vm = unsafe { current_jit_vm() };
    let to = nodes as usize;
    vm.jit_sunk_grow(sunk_site(ops), to / 2, to);
    0
}

/// The sunk site packed into `ops` from bit 32 (its number + 1, 0 for none).
fn sunk_site(ops: i64) -> usize {
    ((ops >> 32) as usize).wrapping_sub(1)
}

/// Materialize a Sinkable site's virtual array slots into
/// a heap `Gc<Table>` at a side-exit emit point. The JIT emit lays
/// out two parallel stack buffers per site per exit (`raws_ptr` of
/// `cap` × u64 and `kinds_ptr` of `cap` × u8, one entry per virt
/// slot) and calls this helper. The caller writes the returned
/// `Value::Table` raw bits into the slot's `reg_state` cell + sets
/// the per-exit-tags entry to `ExitTag::Table` so the dispatcher
/// repacks correctly on deopt.
///
/// `kind` byte uses the same `luna_core::__private::raw::*` tag
/// space as `Value::pack`. Unset slots in `virt_kinds` map to
/// `raw::NIL` at emit time so the table sees a NIL fill — matches
/// Lua's "table created with array part, slot unwritten" semantics.
///
/// # Safety
/// Called from compiled code inside an `enter_jit` window on this thread; `raws_ptr` and
/// `kinds_ptr` point at `cap` readable entries and the three `hash_*` pointers at `n_hash`, entry
/// `i` of each pair being one value's payload and tag, and each hash key an interned string.
// SAFETY: no other item in the link is named `luna_jit_materialize_sunk_table`: only this crate
// defines `luna_jit_` symbols, each once
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_materialize_sunk_table(
    ops: i64,
    cap: i64,
    raws_ptr: *const u64,
    kinds_ptr: *const u8,
    n_hash: i64,
    hash_keys_ptr: *const u64,
    hash_raws_ptr: *const u64,
    hash_kinds_ptr: *const u8,
) -> i64 {
    // SAFETY: inside an enter_jit window (# Safety) JIT_VM is the Vm lent to
    // this call
    let vm = unsafe { current_jit_vm() };
    let cap_u = if cap > 0 { cap as usize } else { 0 };
    let n_hash_u = if n_hash > 0 { n_hash as usize } else { 0 };
    // SAFETY: the buffers are as the caller's contract says (# Safety)
    let sunk = unsafe {
        Sunk::new(
            cap_u,
            raws_ptr,
            kinds_ptr,
            n_hash_u,
            hash_keys_ptr,
            hash_raws_ptr,
            hash_kinds_ptr,
        )
    };
    // sized by the site's `NewTable`, as the interpreter would have made it
    let (b, c, k) = unpack_table_ops(ops);
    let t = vm.jit_sunk_make(sunk_site(ops), |vm| materialize(vm, (b, c, k), &sunk));
    match t {
        Some(g) => g.as_ptr() as i64,
        // a table that cannot be made here is the dispatcher's to make,
        // from its contents kept now, once the trace has returned
        None => vm.jit_sunk_unmade(ops, sunk.arr(), sunk.hash()),
    }
}

/// The contents of a sunk table, in the buffers compiled code laid out.
struct Sunk {
    raws: *const u64,
    kinds: *const u8,
    cap: usize,
    keys: *const u64,
    hash_raws: *const u64,
    hash_kinds: *const u8,
    n_hash: usize,
}

impl Sunk {
    /// # Safety
    /// As [`luna_jit_materialize_sunk_table`]'s buffers, for as long as
    /// the result is used.
    #[allow(clippy::too_many_arguments)]
    unsafe fn new(
        cap: usize,
        raws: *const u64,
        kinds: *const u8,
        n_hash: usize,
        keys: *const u64,
        hash_raws: *const u64,
        hash_kinds: *const u8,
    ) -> Sunk {
        Sunk {
            raws,
            kinds,
            cap,
            keys,
            hash_raws,
            hash_kinds,
            n_hash,
        }
    }

    /// The array values, from index 1.
    fn arr(&self) -> impl ExactSizeIterator<Item = Value> + '_ {
        (0..self.cap).map(|i| {
            // SAFETY: `i < cap`, and the first `cap` entries of the two
            // buffers are readable and hold one live value's payload and
            // tag (`Sunk::new`)
            let v = unsafe {
                Value::pack(
                    *self.kinds.add(i),
                    RawVal {
                        zero: *self.raws.add(i),
                    },
                )
            };
            not_sunk(v)
        })
    }

    /// The hash keys (interned strings) and their values.
    fn hash(&self) -> impl ExactSizeIterator<Item = (Value, Value)> + '_ {
        (0..self.n_hash).map(|i| {
            // SAFETY: `i < n_hash`, and the first `n_hash` entries of the
            // three buffers are readable and hold an interned key and one
            // live value's payload and tag (`Sunk::new`)
            let (key, v) = unsafe {
                (
                    Gc::from_ptr(*self.keys.add(i) as *mut LuaStr),
                    Value::pack(
                        *self.hash_kinds.add(i),
                        RawVal {
                            zero: *self.hash_raws.add(i),
                        },
                    ),
                )
            };
            (Value::Str(key), not_sunk(v))
        })
    }
}

/// `v`, which is no table the exit could not make: a sunk table stored in
/// another makes it escape, so no sunk table holds one.
fn not_sunk(v: Value) -> Value {
    debug_assert!(!matches!(v, Value::Table(t) if t.as_ptr() as usize & 1 == 1));
    v
}

/// The table of [`luna_jit_materialize_sunk_table`], made and filled;
/// `None` when it cannot be.
fn materialize(
    vm: &mut luna_core::vm::Vm,
    (b, c, k): (u32, u32, bool),
    sunk: &Sunk,
) -> Option<Gc<Table>> {
    let g = vm.jit_new_table_presized(b, c, k).ok()??;
    for (i, v) in sunk.arr().enumerate() {
        // SAFETY: `g` was just made in the Vm's heap, which stays in place, and `v` is one of its
        // values
        unsafe { g.store_int(vm.heap_mut(), (i + 1) as i64, v) }.ok()?;
    }
    for (key, v) in sunk.hash() {
        // SAFETY: `g` was just made in the Vm's heap, which stays in place, and `key` and `v` are
        // its values
        unsafe { g.store(vm.heap_mut(), key, v) }.ok()?;
    }
    Some(g)
}
