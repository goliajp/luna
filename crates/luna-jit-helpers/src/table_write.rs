//! Table allocation and store helpers.

use crate::current_jit_vm;

/// Allocate an empty `Gc<Table>` on the active Vm's heap.
/// Returns the Gc pointer pun'd to `i64`. The fresh table is rooted
/// only through the Cranelift Variable the JIT writes it into; no
/// `maybe_collect_garbage` runs inside the helper so the SSA-only
/// rooting suffices for the duration of the JIT entry.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_new_table() -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // A prior helper in this JIT entry parked a deopt
    // request; short-circuit so we don't touch the heap unnecessarily.
    // Returning a NULL ptr is safe because subsequent helpers also
    // early-return on `jit_pending_err`, and the dispatcher will deopt
    // to the interpreter as soon as the JIT entry returns.
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g = vm.heap.new_table();
    g.as_ptr() as i64
}

/// `Heap::new_table_sized(n)` variant. JIT emit reaches
/// for this when the `NewTable` window is immediately followed by a
/// counted `for i = 1, N do … end` with a compile-time-known
/// `N` — pre-allocating the array part skips ~13 intermediate
/// `rehash` rounds for N=10000, which dominates the hot loop's
/// wall-clock on `table_alloc_10k`. Negative or zero hints
/// degrade to an empty table (matches `new_table`).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_new_table_sized(asize: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let n = if asize > 0 { asize as usize } else { 0 };
    let g = vm.heap.new_table_sized(n);
    g.as_ptr() as i64
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
/// `kind` byte uses the same `luna_core::runtime::value::raw::*` tag
/// space as `Value::pack`. Unset slots in `virt_kinds` map to
/// `raw::NIL` at emit time so the table sees a NIL fill — matches
/// Lua's "table created with array part, slot unwritten" semantics.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_materialize_sunk_table(
    cap: i64,
    raws_ptr: *const u64,
    kinds_ptr: *const u8,
    n_hash: i64,
    hash_keys_ptr: *const u64,
    hash_raws_ptr: *const u64,
    hash_kinds_ptr: *const u8,
) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let cap_u = if cap > 0 { cap as usize } else { 0 };
    let n_hash_u = if n_hash > 0 { n_hash as usize } else { 0 };
    let g = vm.heap.new_table_sized(cap_u);
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    // Array slots.
    if cap_u > 0 {
        for i in 0..cap_u {
            // SAFETY: the index is bounded by the buffer length passed as an argument by Cranelift-emitted code, which computes it from the IR's compile-time-known site shape (`n_array_slots` / `n_hash_pairs`).
            let raw_bits = unsafe { *raws_ptr.add(i) };
            // SAFETY: the index is bounded by the buffer length passed as an argument by Cranelift-emitted code, which computes it from the IR's compile-time-known site shape (`n_array_slots` / `n_hash_pairs`).
            let kind = unsafe { *kinds_ptr.add(i) };
            let raw = luna_core::runtime::value::RawVal { zero: raw_bits };
            // SAFETY: `kind` was loaded from the IR-emitted `kinds` buffer in lockstep with the matching raw payload, so the tag byte agrees with the `RawVal` discriminator (see `runtime::value::raw`).
            let v = unsafe { luna_core::runtime::Value::pack(kind, raw) };
            let _ = table.set_int(&mut vm.heap, (i + 1) as i64, v);
        }
    }
    // Hash slots. Each entry is a
    // (key_ptr: *const LuaStr, raw_bits, kind_byte) triple from
    // the trace IR's stack-allocated buffers. The IR baked the
    // const-string ptr at compile time from head_proto.consts.
    if n_hash_u > 0 {
        for i in 0..n_hash_u {
            // SAFETY: the index is bounded by the buffer length passed as an argument by Cranelift-emitted code, which computes it from the IR's compile-time-known site shape (`n_array_slots` / `n_hash_pairs`).
            let key_ptr_bits = unsafe { *hash_keys_ptr.add(i) };
            // SAFETY: the index is bounded by the buffer length passed as an argument by Cranelift-emitted code, which computes it from the IR's compile-time-known site shape (`n_array_slots` / `n_hash_pairs`).
            let raw_bits = unsafe { *hash_raws_ptr.add(i) };
            // SAFETY: the index is bounded by the buffer length passed as an argument by Cranelift-emitted code, which computes it from the IR's compile-time-known site shape (`n_array_slots` / `n_hash_pairs`).
            let kind = unsafe { *hash_kinds_ptr.add(i) };
            let key_gc: luna_core::runtime::Gc<luna_core::runtime::LuaStr> =
                luna_core::runtime::Gc::from_ptr(key_ptr_bits as *mut luna_core::runtime::LuaStr);
            let raw = luna_core::runtime::value::RawVal { zero: raw_bits };
            // SAFETY: `kind` was loaded from the IR-emitted `kinds` buffer in lockstep with the matching raw payload, so the tag byte agrees with the `RawVal` discriminator (see `runtime::value::raw`).
            let v = unsafe { luna_core::runtime::Value::pack(kind, raw) };
            let _ = table.set(&mut vm.heap, luna_core::runtime::Value::Str(key_gc), v);
        }
    }
    g.as_ptr() as i64
}

/// `t[key] = val` where `t` is a Table Gc (i64 pun), `key`
/// is an Int and `val` is an Int. Wraps `Table::set_int(&mut Heap,
/// i64, Value)`. Returns nothing (errors swallowed — luna's
/// `set_int` only returns `Err` on table-size pathology that the
/// interpreter would also surface; JIT'd workloads bounded by N=10k
/// don't reach it). Future caller-visible error reporting would
/// route through a deopt return path.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_int(t: i64, key: i64, val: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    // A metatable on the target table means PUC would route
    // this write through __newindex; the JIT helper would bypass it. Park
    // a deopt request and let the dispatcher re-run the call through the
    // interpreter so __newindex / raw-set semantics are honoured.
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    let _ = table.set_int(&mut vm.heap, key, luna_core::runtime::Value::Int(val));
}

/// Write an arbitrary `Value::pack(tag, raw_bits)` to
/// `t[key]` (Int key). Generalises `_table_set_int` / `_table_set_nil`:
/// trace JIT emit dispatches Int/Nil to their specialized helpers
/// (slightly less overhead) and Closure/Table/Float/etc. to this
/// helper. Without it, a SetTable whose src is a Closure (from
/// Op::Closure trace JIT) silently wraps the closure pointer as
/// `Value::Int(ptr_bits)` — a number that later calls fail with
/// "attempt to call a number value".
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_table_set_raw(t: i64, key: i64, raw_bits: i64, tag: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    // SAFETY: `kind` was loaded from the IR-emitted `kinds` buffer in lockstep with the matching raw payload, so the tag byte agrees with the `RawVal` discriminator (see `runtime::value::raw`).
    let v = unsafe {
        luna_core::runtime::Value::pack(
            tag as u8,
            luna_core::runtime::value::RawVal {
                zero: raw_bits as u64,
            },
        )
    };
    let _ = table.set_int(&mut vm.heap, key, v);
    barrier_for(vm, g, luna_core::runtime::Value::Int(key), v);
}

/// Write `Value::pack(tag, raw)` to `t[key_ptr_as_str]`.
/// String key is a `Gc<LuaStr>` raw pointer (baked into IR at
/// emit time from `head_proto.consts[ins.b()]`); value goes
/// through the standard tag/raw round-trip. Used for Op::SetField
/// trace JIT support (helper path; the sunk emit path is separate).
///
/// Same metatable / pending_err short-circuit as the other table
/// helpers — `__newindex` cases deopt to interp.
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_table_set_field(
    t: i64,
    key_ptr: i64,
    val_raw: i64,
    val_tag: i64,
) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return;
    }
    let key_gc: luna_core::runtime::Gc<luna_core::runtime::LuaStr> =
        luna_core::runtime::Gc::from_ptr(key_ptr as *mut luna_core::runtime::LuaStr);
    let key = luna_core::runtime::Value::Str(key_gc);
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    // SAFETY: `kind` was loaded from the IR-emitted `kinds` buffer in lockstep with the matching raw payload, so the tag byte agrees with the `RawVal` discriminator (see `runtime::value::raw`).
    let v = unsafe {
        luna_core::runtime::Value::pack(
            val_tag as u8,
            luna_core::runtime::value::RawVal {
                zero: val_raw as u64,
            },
        )
    };
    let _ = table.set(&mut vm.heap, key, v);
    barrier_for(vm, g, key, v);
}

/// The trace JIT's table stores, `t[key] = val`, with the value given as
/// a `raw` tag and payload. They return `1` when stored. They store
/// nothing and return `0` when the table has a metatable, whose
/// `__newindex` the helper would bypass, or when the key cannot index a
/// table (nil, NaN); the caller then side-exits at the storing op and
/// the interpreter performs it. The trace goes no further than that op,
/// so nothing it did before is repeated.
///
/// # Safety
/// `t` is a live table, and `val_tag` is the tag of a register holding
/// `val_raw` (see `runtime::value::raw`).
#[inline]
unsafe fn checked_store(t: i64, key: luna_core::runtime::Value, val_raw: i64, val_tag: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.counters.deopt += 1;
        return 0;
    }
    // SAFETY: the caller passes a register's tag with its payload.
    let val = unsafe {
        luna_core::runtime::Value::pack(
            val_tag as u8,
            luna_core::runtime::value::RawVal {
                zero: val_raw as u64,
            },
        )
    };
    // SAFETY: `t` is a live table the trace holds in a register.
    let table = unsafe { g.as_mut() };
    if table.set(&mut vm.heap, key, val).is_err() {
        vm.jit.counters.deopt += 1;
        return 0;
    }
    barrier_for(vm, g, key, val);
    1
}

/// The write barrier for a store of `key` / `val` into `g`, as the
/// interpreter's stores take it: a collectable one in a table the
/// collector already traced (black) sends the table back to be traced
/// again, or the new object would be swept while the table holds it.
fn barrier_for(
    vm: &mut luna_core::vm::Vm,
    g: luna_core::runtime::Gc<luna_core::runtime::Table>,
    key: luna_core::runtime::Value,
    val: luna_core::runtime::Value,
) {
    use luna_core::runtime::value::raw;
    if raw::is_gc(val.unpack().0) || raw::is_gc(key.unpack().0) {
        vm.heap
            .barrier_back(g.as_ptr() as *mut luna_core::runtime::heap::GcHeader);
    }
}

/// `t[key] = val` with an integer key; see `checked_store`.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_int_checked(
    t: i64,
    key: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    // SAFETY: see `checked_store`.
    unsafe { checked_store(t, luna_core::runtime::Value::Int(key), val_raw, val_tag) }
}

/// `t[key] = val` with an interned string key; see `checked_store`.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_field_checked(
    t: i64,
    key_ptr: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    let key: luna_core::runtime::Gc<luna_core::runtime::LuaStr> =
        luna_core::runtime::Gc::from_ptr(key_ptr as *mut luna_core::runtime::LuaStr);
    // SAFETY: see `checked_store`.
    unsafe { checked_store(t, luna_core::runtime::Value::Str(key), val_raw, val_tag) }
}

/// `t[key] = val` with a key of any type, given like the value; see
/// `checked_store`.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_checked(
    t: i64,
    key_raw: i64,
    key_tag: i64,
    val_raw: i64,
    val_tag: i64,
) -> i64 {
    // SAFETY: the trace passes a register's tag with its payload.
    let key = unsafe {
        luna_core::runtime::Value::pack(
            key_tag as u8,
            luna_core::runtime::value::RawVal {
                zero: key_raw as u64,
            },
        )
    };
    // SAFETY: see `checked_store`.
    unsafe { checked_store(t, key, val_raw, val_tag) }
}

/// Write `Value::Nil` to `t[key]` (Int key). Used by
/// trace JIT when a SetList/SetI/SetTable's source register is a
/// `RegKind::Nil` (e.g. Lua's `local t = {nil, nil}` table
/// constructor expands to `NewTable; LoadNil×N; SetList` and
/// without a Nil-specific helper the existing `_table_set_int`
/// would silently coerce the Nil to `Value::Int(0)`).
///
/// Same metatable / `jit_pending_err` short-circuit as the other
/// `_table_set_*` helpers — caller deopts on `pending_err` and
/// the interpreter re-runs the op to honour `__newindex`.
///
/// No compiler emits this call any more: the trace JIT stores through
/// the `luna_jit_table_set_*_checked` helpers. It stays for the 3.x API.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_nil(t: i64, key: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    let _ = table.set_int(&mut vm.heap, key, luna_core::runtime::Value::Nil);
}

/// Float-key, Float-value variant. luna 5.1 / 5.2 lower
/// `for i = 1, N do t[i] = i end` with a Float loop var (no Int
/// subtype in those dialects), so the SetTable's key and value
/// arguments arrive as f64 bit-patterns. `Table::set` normalizes
/// integral Float keys back to Int slots so `#t` still reports the
/// array length we'd expect — same shape PUC produces.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_set_float_float(t: i64, key_bits: i64, val_bits: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let table = unsafe { g.as_mut() };
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    let v = luna_core::runtime::Value::Float(f64::from_bits(val_bits as u64));
    // a NaN key raises; the interpreter re-runs the call and reports it
    if table.set(&mut vm.heap, k, v).is_err() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: invalid table key"));
    }
}
