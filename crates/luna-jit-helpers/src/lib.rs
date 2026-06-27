//! v2.1 Phase 1K.D.1 — shared `luna_jit_*` extern-C runtime helpers
//! and the per-thread `JIT_VM` / `JIT_CL` TLS slots plus the
//! `enter_jit` RAII rebind. Extracted verbatim from
//! `luna-jit/src/jit_backend/mod.rs` so both `luna-jit` (Cranelift)
//! and `luna-jit-llvm` (v2.1 alt backend) share one symbol table
//! and one TLS discipline.
//!
//! luna-jit re-exports everything in this crate via
//! `pub use luna_jit_helpers::*;` from `jit_backend/mod.rs`, so all
//! historical `crate::jit_backend::luna_jit_*` / `super::luna_jit_*`
//! paths resolve unchanged. luna-jit-llvm depends on this crate
//! directly without pulling Cranelift.
//!
//! See `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md` § 3.5 + § 5.1 for
//! the extraction rationale.
//!
//! # Invariants
//!
//! - Symbol names are `#[unsafe(no_mangle)] pub unsafe extern "C" fn
//!   luna_jit_*` — Cranelift's `Linkage::Import` resolves them by
//!   linker symbol; LLVM's `Module::add_function` resolves them via
//!   JIT execution-engine `add_global_mapping`.
//! - Every helper is called only under an active `enter_jit` guard
//!   (which pins `JIT_VM` / `JIT_CL` for the dispatch window) and
//!   reads the Vm/closure pointer via `current_jit_vm()` /
//!   `current_jit_closure()`.

// All helpers use fully-qualified `luna_core::*` paths internally
// (preserved verbatim from the original `luna-jit/src/jit_backend/mod.rs`
// site). Only the `JitVmGuard` re-export is needed by the `enter_jit`
// signature below.
use luna_core::jit::JitVmGuard;

thread_local! {
    /// v2.0 Track J sub-step J-B — `JIT_CACHE` (Phase D) and
    /// `JIT_CACHE_HANDLES` (Phase E) both migrated to
    /// `Vm.jit.storage.{cache,cache_handles}`. The JIT_VM / JIT_CL
    /// per-dispatch slots below stay TLS until J-D's
    /// `scoped_jit_vm_rebind` RAII lift.

    /// P11-S5c — current `Vm` pointer for Rust helpers called from
    /// JIT'd code. Set by [`enter_jit`] just before invoking the
    /// entry fn; cleared (RAII via [`JitVmGuard`]) on return. Helpers
    /// (`luna_jit_new_table`, `luna_jit_table_set_int`, etc.) read
    /// this to reach `Vm.heap`. Null when no JIT call is in flight.
    static JIT_VM: std::cell::Cell<*mut luna_core::vm::Vm> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
    /// P11-S5d.J — current `LuaClosure` pointer for `Op::GetUpval`
    /// value-read helpers. Set alongside `JIT_VM` by [`enter_jit`].
    /// Null when no JIT call is in flight, or when the active call
    /// has no upvalues (zero-upval Protos never reach
    /// `luna_jit_upval_get`).
    static JIT_CL: std::cell::Cell<*const luna_core::runtime::LuaClosure> =
        const { std::cell::Cell::new(std::ptr::null()) };
}

/// P11-S5c — install `vm` as the current JIT Vm pointer. Returns a
/// [`JitVmGuard`] whose drop restores the prior `(JIT_VM, JIT_CL)`
/// values (J-D RAII rebind). Must be held across the JIT entry-fn
/// call so any helper can pick the pointer up.
///
/// The guard type itself lives in `luna_core::jit` so the trait
/// signature in `IntChunkCompiler::enter` doesn't drag Cranelift into
/// luna-core.
///
/// # v2.0 Track J sub-step J-D — capture-and-restore
///
/// Before J-D the body just overwrote the TLS slots and returned an
/// inert guard ([`noop_jit_guard`]); the "next `enter_jit` overwrites
/// anyway" invariant made the elision harmless under single-thread,
/// single-level dispatch. Cross-thread Vm move plus nested JIT entry
/// (e.g. JIT'd op → metamethod → Lua-from-Rust → JIT entry again)
/// makes the no-op-drop variant unsafe: the outer entry would resume
/// holding the inner Vm's slot. J-D therefore delegates to
/// [`scoped_rebind::scoped_jit_vm_rebind`], which snapshots the
/// previous values into the guard and restores them on drop.
///
/// P11-S5d.J — the `cl` parameter is the closure being invoked. The
/// guard also pins it in `JIT_CL` so `luna_jit_upval_get` can fetch
/// `cl.upvals[idx]` at runtime. Callers that don't need upvalues (the
/// zero-arg host-call path before `Op::GetUpval` was JIT'd) can pass
/// `None`; helpers will hit the debug-assert if they fire.
pub fn enter_jit(
    vm: &mut luna_core::vm::Vm,
    cl: Option<luna_core::runtime::Gc<luna_core::runtime::LuaClosure>>,
) -> JitVmGuard {
    scoped_rebind::scoped_jit_vm_rebind(vm, cl)
}

/// v2.0 Track J sub-step J-D — test-only inspector of the active
/// `(JIT_VM, JIT_CL)` TLS pointers. Used by the J-D regression test
/// (`tests/j_d_scoped_rebind_and_sleeve.rs`) to assert RAII install +
/// restore semantics across nested [`enter_jit`] calls. Not part of
/// the embedder API.
#[doc(hidden)]
pub fn __j_d_tls_ptrs() -> (
    *mut luna_core::vm::Vm,
    *const luna_core::runtime::LuaClosure,
) {
    let vm = JIT_VM.with(|c| c.get());
    let cl = JIT_CL.with(|c| c.get());
    (vm, cl)
}

/// P11-S5c — read the active Vm pointer. SAFETY: the caller (always
/// a Rust helper invoked from inside JIT'd code) must be running
/// under an active [`enter_jit`] guard.
#[inline]
unsafe fn current_jit_vm<'a>() -> &'a mut luna_core::vm::Vm {
    let p = JIT_VM.with(|cell| cell.get());
    debug_assert!(!p.is_null(), "JIT helper called outside enter_jit scope");
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { &mut *p }
}

/// P11-S5d.J — read the active LuaClosure pointer. SAFETY: caller is
/// a JIT helper running under an `enter_jit` guard whose closure
/// argument was non-None.
#[inline]
unsafe fn current_jit_closure() -> luna_core::runtime::Gc<luna_core::runtime::LuaClosure> {
    let p = JIT_CL.with(|cell| cell.get());
    debug_assert!(
        !p.is_null(),
        "luna_jit_upval_get called outside an upval-aware enter_jit scope"
    );
    luna_core::runtime::Gc::from_ptr(p as *mut luna_core::runtime::LuaClosure)
}

/// P11-S5c — allocate an empty `Gc<Table>` on the active Vm's heap.
/// Returns the Gc pointer pun'd to `i64`. The fresh table is rooted
/// only through the Cranelift Variable the JIT writes it into; no
/// `maybe_collect_garbage` runs inside the helper so the SSA-only
/// rooting suffices for the duration of the JIT entry.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_new_table() -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // P11-S5d.E' — a prior helper in this JIT entry parked a deopt
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

/// P11-S5c.B — `Heap::new_table_sized(n)` variant. JIT emit reaches
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

/// P12-S5-C — materialize a Sinkable site's virtual array slots into
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
    // P12-S11-B-v2 — hash slots. Each entry is a
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

/// P11-S5c — `t[key] = val` where `t` is a Table Gc (i64 pun), `key`
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
    // P11-S5d.E' — a metatable on the target table means PUC would route
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

/// P12-S7-C — write an arbitrary `Value::pack(tag, raw_bits)` to
/// `t[key]` (Int key). Generalises `_table_set_int` / `_table_set_nil`:
/// trace JIT emit dispatches Int/Nil to their specialized helpers
/// (slightly less overhead) and Closure/Table/Float/etc. to this
/// helper. Without it, a SetTable whose src is a Closure (post-S7
/// Op::Closure trace JIT) silently wraps the closure pointer as
/// `Value::Int(ptr_bits)` — a number that later calls fail with
/// "attempt to call a number value".
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
}

/// P12-S11-A — write `Value::pack(tag, raw)` to `t[key_ptr_as_str]`.
/// String key is a `Gc<LuaStr>` raw pointer (baked into IR at
/// emit time from `head_proto.consts[ins.b()]`); value goes
/// through the standard tag/raw round-trip. Used for Op::SetField
/// trace JIT support (helper path; sunk emit is S11-B).
///
/// Same metatable / pending_err short-circuit as the other table
/// helpers — `__newindex` cases deopt to interp.
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
}

/// P12-S11-A — read `t[key_ptr_as_str]` and return raw payload bits.
/// String key is a `Gc<LuaStr>` raw pointer baked into IR. Caller
/// (trace JIT GetField emit) infers exit_tag for the dst slot via
/// `infer_getx_exit`; absent inference, dispatchable=false.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_table_get_field(t: i64, key_ptr: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    let key_gc: luna_core::runtime::Gc<luna_core::runtime::LuaStr> =
        luna_core::runtime::Gc::from_ptr(key_ptr as *mut luna_core::runtime::LuaStr);
    let v = g.get(luna_core::runtime::Value::Str(key_gc));
    let (_tag, raw) = v.unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// v1.2 D3 Path B — read `upvals[upval_idx][key_str]` and return raw
/// payload bits. Mirrors `luna_jit_table_get_field` but resolves the
/// table via the trace head closure's upvalue list first (the trace
/// dispatcher's `enter_jit(vm, Some(cl))` pins `JIT_CL`).
///
/// Used by the trace JIT lowerer's `Op::GetTabUp` arm for upvalue-
/// table accesses outside the recognised math-fold pattern. The
/// canonical case is `math.min(a, b)` whose 2-arg shape doesn't
/// match `try_match_trace_math_fold`'s single-arg libm catalog;
/// without this helper the entire trace bails at the `cmp-dirs`
/// pre-emit pass and the workload runs interp-only (P3a diag finding
/// 2026-06-24: `bail:cmp-dirs-GetTabUp` × 200/200 on `token_bucket_1k`).
///
/// Deopt cases: upval isn't a Table (corrupted upval list) or has
/// a metatable (`__index` could shadow the lookup — interp-only).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_op_get_tab_up(upval_idx: i64, key_ptr: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code; `enter_jit(vm, Some(cl))` pinned JIT_CL for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let env = vm.upval_get(cl, upval_idx as u32);
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> = match env {
        luna_core::runtime::Value::Table(t) => t,
        _ => {
            vm.jit.pending_err = Some(vm.rt_err("JIT deopt: GetTabUp upval not Table"));
            return 0;
        }
    };
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: GetTabUp env has metatable"));
        return 0;
    }
    let key_gc: luna_core::runtime::Gc<luna_core::runtime::LuaStr> =
        luna_core::runtime::Gc::from_ptr(key_ptr as *mut luna_core::runtime::LuaStr);
    let v = g.get(luna_core::runtime::Value::Str(key_gc));
    let (_tag, raw) = v.unpack();
    // SAFETY: pulled from `RawVal` of a freshly unpacked Value above.
    unsafe { raw.zero as i64 }
}

/// P12-S6-A2 — write `Value::Nil` to `t[key]` (Int key). Used by
/// trace JIT when a SetList/SetI/SetTable's source register is a
/// `RegKind::Nil` (e.g. Lua's `local t = {nil, nil}` table
/// constructor expands to `NewTable; LoadNil×N; SetList` and
/// without a Nil-specific helper the existing `_table_set_int`
/// would silently coerce the Nil to `Value::Int(0)`).
///
/// Same metatable / `jit_pending_err` short-circuit as the other
/// `_table_set_*` helpers — caller deopts on `pending_err` and
/// the interpreter re-runs the op to honour `__newindex`.
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

/// P11-S5c — Float-key, Float-value variant. luna 5.1 / 5.2 lower
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
    let _ = table.set(&mut vm.heap, k, v);
}

/// P11-S5c — `t[key]` where the JIT statically expects an Int
/// result. Pulls the raw `Value` from the table and unpacks
/// the Int payload. If the slot is anything but Int (Nil, Float,
/// Str, …) the helper returns 0 — the JIT scan only admits
/// chunks that store Ints, so the divergence is observable only
/// when the user-facing semantics violate the static expectation.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_int(t: i64, key: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    // P11-S5d.E' — metatable on the source table means PUC would route
    // a missing entry through __index; the helper bypasses that. Park a
    // deopt request and bail; the dispatcher re-runs the call through
    // the interpreter, which walks __index correctly (including the
    // infinite-loop error events.lua relies on).
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    // P11-S5d.B — return the raw 8-byte payload of the stored
    // Value, regardless of tag. The JIT-emitted caller interprets
    // the bit pattern according to the GetI result's RegKind:
    // Int → i64, Float → f64::from_bits, Table → Gc<Table>::from_ptr.
    // A previous variant unconditionally returned 0 on non-Int /
    // non-Float — that fed NULL into subsequent helpers when the
    // table actually stored Gc objects (binary_trees' `check`
    // chain calling itself on `t[1]`).
    let v = g.get_int(key);
    let (_tag, raw) = v.unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// P11-S5d.E' — `t[k]` where `k` is a Float key. luna 5.1 / 5.2's
/// `OP_GETTABLE` typically loads the key via `LoadF` (no Int subtype
/// in those dialects); the emit hands `k` as `f64::to_bits` so the
/// helper can reconstruct the Float value before calling `Table::get`.
/// `Table::get` normalises integral Floats back to the Int slot, so
/// `t[1.0]` lands on `t[1]` exactly like PUC does. Returns the raw
/// 8-byte payload (same convention as `luna_jit_table_get_int`).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_get_float(t: i64, key_bits: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    let k = luna_core::runtime::Value::Float(f64::from_bits(key_bits as u64));
    let v = g.get(k);
    let (_tag, raw) = v.unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// P11-S5d.J — `R[A] = upvals[idx]` value-read variant. Reads the
/// active closure's upvalue cell, dispatching open/closed via the
/// interpreter's `Vm::upval_get` (so an open upvalue resolves to its
/// current stack slot — matters when a closure is called from inside
/// an enclosing function whose upvalues are still open). Returns the
/// raw 8-byte payload (same convention as the table helpers): the
/// JIT-emitted caller bitcasts to F64 if the slot's declared kind is
/// Float, leaves as I64 otherwise.
///
/// Scope: only invoked for `Op::GetUpval` PCs the scan classified as
/// `ValueRead` (not the self-recursion call-target marker). The
/// dispatcher pins `JIT_CL` at entry; helper safety relies on that.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_upval_get(idx: i64) -> i64 {
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let v = vm.upval_get(cl, idx as u32);
    let (_tag, raw) = v.unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// P12-S7-C — trace JIT helper for `Op::Close A`. Wraps
/// `Vm::jit_op_close` which does the predict-and-deopt logic:
/// returns 0 to continue the trace, 1 to deopt (handler would run
/// or pre-existing pending_err).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_close(start_offset: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_close(start_offset as u32)
}

/// P12-S12-C v1 — update only the raw payload of
/// `vm.stack[base + slot_offset]`, preserving its existing tag.
/// Used by `Op::Concat` body emit to spill trace-IR Variables
/// back to vm.stack for operands whose `current_kinds` is
/// `Unset` (e.g. Str slots that round-trip as pointer raw bits
/// but have no `RegKind::Str` variant). The interp's previous
/// execution of the same op already wrote the right `tag` to
/// that slot — the trace just needs to refresh the raw bits.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_stack_update_raw(slot_offset: i64, raw_bits: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    vm.jit_stack_update_raw(slot_offset as u32, raw_bits as u64);
}

/// P12-S12-C v1 — trace JIT helper for `Op::Concat A B`.
///
/// Wraps `Vm::jit_op_concat` which mirrors the interp arm: sets
/// `self.top = base + a + n`, then runs `concat_run(base + a)`.
/// Detects metamethod-path (which would push a Lua frame mid-trace)
/// via pre/post `frames.len()` comparison and deopts cleanly via
/// `pending_err` + frame unwind.
///
/// Returns `0` on success (result lives at `vm.stack[base + a]`),
/// `-1` on deopt (pending_err set; metamethod path, type error,
/// length overflow, or pre-existing pending_err).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_op_concat(slot_offset: i64, n: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_concat(slot_offset as u32, n as i32)
}

/// P14-S14-B v2 — trace JIT helper:acquire a fresh accumulator
/// buffer from the Vm's pool. Returns a `*mut Vec<u8>` boxed-leaked
/// pointer that the trace fn keeps in a stack slot through the loop.
///
/// Safety: caller must be inside `enter_jit` and must eventually call
/// `luna_jit_str_buf_release` with the returned pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_acquire() -> i64 {
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_acquire() as i64
}

/// P14-S14-B v2 — trace JIT helper:release a buffer back to the
/// Vm's pool.
///
/// Safety: `buf` must have been returned by a prior
/// `luna_jit_str_buf_acquire` on the same Vm.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_release(buf: i64) {
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_release(buf as *mut Vec<u8>);
}

/// P14-S14-B v2 — trace JIT helper:append a LuaStr's bytes to a
/// previously-acquired accumulator buffer. The trace IR calls this
/// at each loop iter inside the `s = s .. v` idiom.
///
/// Returns 0 on success, -1 if `str_ptr` isn't a valid LuaStr (deopt
/// to interp, which will hit the __concat metamethod path).
///
/// Safety: `buf` from prior `acquire`; `str_ptr` from the piece slot.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_extend(buf: i64, str_ptr: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_extend(buf as *mut Vec<u8>, str_ptr)
}

/// P14-S14-B v2 — trace JIT helper:drain the accumulator buffer
/// into a fresh `LuaStr` via `heap.intern`, returning the raw ptr
/// bits for the trace to write into the accumulator slot.
///
/// Returns the LuaStr ptr as i64 on success, 0 on overflow (the v2
/// hard cap = 256KB; trace deopts).
///
/// Safety: `buf` from prior `acquire`. The buffer is drained and
/// ready for `release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_str_buf_intern(buf: i64) -> i64 {
    let vm = unsafe { current_jit_vm() };
    vm.jit_str_buf_intern(buf as *mut Vec<u8>)
}

/// P12-S12-B-v2 — trace JIT helper for `Op::TForCall A 0 C`.
///
/// Mirrors `exec.rs:5316` Op::TForCall semantics:
/// - copies `R[A..=A+2]` (iter / state / control) to `R[A+4..=A+6]`,
///   resizing `vm.stack` if needed
/// - calls `vm.begin_call(abs+4, Some(2), nvars, false)` to dispatch
///   the iterator function
///
/// v2 restriction: the iterator at `R[A]` must be `Value::Native`. A
/// Lua-closure iter would push a Lua frame mid-trace, breaking the
/// trace head's `recording_frame_base` invariant; we deopt instead
/// (sets `jit_pending_err`, returns sentinel). The expected v3
/// follow-up inlines `inext` directly so the helper path is gone.
///
/// Returns `0` on success, `-1` on deopt (pending_err set OR
/// pre-existing pending_err).
///
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_tforcall(
    abs_offset: i64,
    nvars: i64,
    ctrl_out: *mut i64,
    key_out: *mut i64,
    val_out: *mut i64,
) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_op_tforcall(abs_offset as u32, nvars as i32, ctrl_out, key_out, val_out)
}

/// P12-S12-B-v2 — load the raw `i64` payload of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used to reload trace IR
/// `Variable`s after a helper (e.g. `luna_jit_op_tforcall`) has
/// mutated `vm.stack` directly.
///
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live. Returns `0` if the slot is out of
/// stack range (defensive — emit-time bounds check should make this
/// unreachable).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_load(slot_offset: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    vm.jit_stack_load(slot_offset as u32)
}

/// P12-S12-B-v2 — read the tag byte of `vm.stack[base + slot_offset]`
/// for the active trace's head frame. Used by `Op::TForLoop` emit
/// to dispatch on the iterator's return-key tag (Nil → loop end,
/// Int → continue for ipairs, other → deopt for v2).
///
/// Safety: caller (trace JIT IR) runs under `enter_jit`. Returns
/// `raw::NIL` (0) if slot out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_stack_tag(slot_offset: i64) -> i64 {
    let vm = unsafe { current_jit_vm() };
    vm.jit_stack_tag(slot_offset as u32) as i64
}

/// P12-S7-B — spill a trace's per-register live value into the
/// caller frame's `vm.stack[base + slot_offset]`. Always called
/// just before `luna_jit_op_closure` for each `in_stack: true`
/// upval in the inner proto, so the open upval the helper creates
/// points to a slot holding the right value.
///
/// Parameters: `slot_offset` is the caller-frame register index
/// (`u32`, depth=0 only — S7-B doesn't support depth>0 Closure).
/// `tag` is the `raw::*` byte for the register's RegKind at this
/// emit point (Int / Float / Table / Closure / Nil). `raw_bits` is
/// the trace IR's i64 payload for the register (Float held as
/// `f64::to_bits`, Table/Closure as raw `Gc::as_ptr` cast).
///
/// Safety: caller (trace JIT IR) runs under `enter_jit` so
/// `current_jit_vm()` is live; the (tag, raw_bits) pair is
/// generated by the same emit path that proves the kind, so
/// `Value::pack` round-trips correctly.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_spill_to_stack(slot_offset: i64, tag: i64, raw_bits: i64) {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return;
    }
    vm.jit_spill_stack(slot_offset as u32, tag as u8, raw_bits as u64);
}

/// P12-S7-A — trace JIT helper for `Op::Closure A Bx`.
///
/// Looks up `cl.proto.protos[bx]` (the inner Proto) and builds a
/// new `Gc<LuaClosure>` for it. Each upval is captured either from
/// the trace head closure's `upvals()` slice (`in_stack=false`)
/// or from the caller frame's stack via `find_or_create_upval`
/// (`in_stack=true`, P12-S7-B). v51 dialect clones the `_ENV` cell
/// to match interp semantics (per-closure `_ENV`). v52+ honours
/// the Proto cache.
///
/// **Pre-condition for in_stack upvals**: the trace IR has already
/// emitted `luna_jit_spill_to_stack(d.index, tag, raw)` for every
/// `d.in_stack == true` upval BEFORE this call, so the underlying
/// `vm.stack[base + d.index]` holds the trace's current value at
/// helper time. Without that spill the open upval would point at
/// a stale entry-tag value.
///
/// Returns the raw `Gc<LuaClosure>` ptr as i64 (Value::Closure's
/// payload). On error (`pending_err` already set) returns 0
/// sentinel so the dispatcher deopts.
///
/// Safety: caller runs under `enter_jit(vm, Some(cl))` guard so
/// `current_jit_vm()` / `current_jit_closure()` return live
/// references. `proto_idx` is in-bounds by the emit pre-check.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_op_closure(proto_idx: i64) -> i64 {
    use luna_core::runtime::function::{INLINE_UPVALS_N, UpvalState, Upvalue};
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let inner = cl.proto.protos[proto_idx as usize];
    let n_ups = inner.upvals.len();
    // Determine the caller frame's base for in_stack captures. The
    // helper runs MID-trace, before any frame writeback — the trace
    // head's frame is the topmost Lua frame here (S7-B restricts
    // Op::Closure emit to inline_depth=0 only, so no deeper frame
    // exists).
    let base = match vm.jit_last_lua_frame() {
        Some(f) => f.base,
        None => {
            vm.jit.pending_err = Some(vm.rt_err("JIT op_closure: no Lua frame"));
            return 0;
        }
    };
    // Build the upval slice — small (0..2 typical) so use a stack
    // array up to INLINE_UPVALS_N like the interp does, else heap.
    let mut stack_buf: [std::mem::MaybeUninit<luna_core::runtime::Gc<Upvalue>>; INLINE_UPVALS_N] =
        [std::mem::MaybeUninit::uninit(); INLINE_UPVALS_N];
    let mut heap_buf: Vec<luna_core::runtime::Gc<Upvalue>> = Vec::new();
    let use_inline = n_ups <= INLINE_UPVALS_N;
    if !use_inline {
        heap_buf.reserve_exact(n_ups);
    }
    for (i, d) in inner.upvals.iter().enumerate() {
        let uv = if d.in_stack {
            // P12-S7-B — `find_or_create_upval` points the open
            // upval at vm.stack[base + d.index]. The trace IR
            // emitted a spill before this call, so the slot holds
            // the right value at capture time.
            vm.find_or_create_upval(base + d.index as u32)
        } else {
            cl.upvals()[d.index as usize]
        };
        if use_inline {
            stack_buf[i] = std::mem::MaybeUninit::new(uv);
        } else {
            heap_buf.push(uv);
        }
    }
    let ups: &mut [luna_core::runtime::Gc<Upvalue>] = if use_inline {
        // SAFETY: first n_ups slots of stack_buf were initialised
        // by the loop above; we expose exactly that range.
        unsafe {
            std::slice::from_raw_parts_mut(
                stack_buf.as_mut_ptr() as *mut luna_core::runtime::Gc<Upvalue>,
                n_ups,
            )
        }
    } else {
        &mut heap_buf[..]
    };
    // v51 per-closure `_ENV` clone — matches interp Op::Closure.
    let v51 = vm.version() <= luna_core::version::LuaVersion::Lua51;
    if v51 && inner.env_upval_idx != u8::MAX {
        let i = inner.env_upval_idx as usize;
        let cur = match ups[i].state() {
            UpvalState::Open { slot, thread } => vm.read_slot(slot, thread),
            UpvalState::Closed(v) => v,
        };
        ups[i] = vm.heap.new_upvalue(UpvalState::Closed(cur));
    }
    let ups_slice: &[luna_core::runtime::Gc<Upvalue>] = ups;
    let nc = if v51 {
        vm.heap.new_closure_inline(inner, ups_slice)
    } else {
        // PUC 5.2+ getcached: reuse the last LuaClosure built for
        // this Proto if every upval slot points to the same
        // Upvalue object (typical for `function() return outer end`
        // captured inside a hot loop).
        let cached = inner.cache.get().filter(|c| {
            c.upvals().len() == ups_slice.len()
                && c.upvals()
                    .iter()
                    .zip(ups_slice.iter())
                    .all(|(a, b)| std::ptr::eq(a.as_ptr(), b.as_ptr()))
        });
        match cached {
            Some(c) => c,
            None => {
                let n = vm.heap.new_closure_inline(inner, ups_slice);
                inner.cache.set(Some(n));
                n
            }
        }
    };
    let (_tag, raw) = luna_core::runtime::Value::Closure(nc).unpack();
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    unsafe { raw.zero as i64 }
}

/// v2.0 Phase 5 Track AO sub-track AO-PF — runtime fire counter for
/// the Stage 7 polish 6 inline-chain reloc path. Every call to
/// [`luna_jit_trace_materialize_frames`] from trace mcode (JIT-baked
/// OR AOT polish-6 slot-loaded) increments this counter. In an AOT-
/// only run (no in-process JIT compilation of traces that carry
/// inline cmp@d>0 side-exits) any non-zero value is direct evidence
/// that the polish-6 chain reloc path actually fires at runtime — the
/// resolver-side probe (`aot_inline_chains_resolved`) only confirms
/// the slot was populated, not that any AOT mcode dispatch ever
/// loaded it. See `.dev/rfcs/v2.0-ao-pf-verdict.md`.
pub static TRACE_MATERIALIZE_FRAMES_FIRES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Reader for [`TRACE_MATERIALIZE_FRAMES_FIRES`]. Relaxed load is fine
/// — the counter is diagnostic, not a synchronisation point.
pub fn trace_materialize_frames_fires() -> u64 {
    TRACE_MATERIALIZE_FRAMES_FIRES.load(std::sync::atomic::Ordering::Relaxed)
}

/// P12-S4-step4b — frame materialization helper.
///
/// step4b-B body: walks `metas[0..n]` and pushes one
/// `CallFrame::Lua` per entry onto `vm.frames` so the interp can
/// resume at a depth>0 continuation PC after the trace side-exits.
/// Returns `0` on success, non-zero to force the dispatcher into
/// the deopt path. The lowerer (step4b-C) will emit the call site
/// from cmp@d>0 side-exit blocks.
///
/// Invariants the caller (lowerer) enforces at compile time:
/// - All inlined frames are the same `LuaClosure` (self-recursion
///   only), so `current_jit_closure()` matches every frame's
///   closure pointer.
/// - The chain is non-vararg (`!cl.proto.is_vararg`) — helper does
///   NOT reconstruct the vararg rotation that `push_frame` does.
/// - Every inlined `Op::Call` has `C == 2` (one return value);
///   `m.nresults` is therefore always 1. The helper writes whatever
///   the metadata says, no validation.
///
/// Safety:
/// - Caller runs under an `enter_jit(vm, Some(cl))` guard so
///   `current_jit_vm()` / `current_jit_closure()` return live
///   references.
/// - `metas` points to a valid array of length `n` of
///   `FrameMaterializeInfo`, alive for the duration of the call —
///   today it's a pointer into the owning `CompiledTrace.frame_metas`
///   `Box`, which lives at least as long as the trace's mmap.
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
// SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
pub unsafe extern "C" fn luna_jit_trace_materialize_frames(
    n: u64,
    metas: *const luna_core::jit::trace::FrameMaterializeInfo,
) -> i64 {
    // AO-PF — count every entry to this helper from trace mcode.
    // Relaxed ordering: the counter is purely diagnostic; the read
    // side runs after process work has quiesced.
    TRACE_MATERIALIZE_FRAMES_FIRES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    // Honour the existing deopt protocol: if any earlier helper in
    // this JIT entry parked a deopt, don't push frames — the
    // dispatcher will unwind via the deopt path.
    if vm.jit.pending_err.is_some() {
        return -1;
    }
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let cl = unsafe { current_jit_closure() };
    let head_frame = match vm.jit_last_lua_frame() {
        Some(f) => f,
        // No live Lua frame at trace head — shouldn't happen under
        // any current dispatcher path, but treat as deopt rather
        // than panic from the JIT.
        None => return -1,
    };
    let max_stack = cl.proto.max_stack as u32;
    for i in 0..n as usize {
        // SAFETY: caller-supplied `metas` points to a valid array of
        // length `n` per the contract above.
        let m = unsafe { *metas.add(i) };
        let new_base = head_frame.base + m.base_offset;
        vm.jit_ensure_stack((new_base + max_stack) as usize);
        vm.jit_push_inlined_frame(cl, new_base, m.pc, m.nresults);
    }
    0
}

/// P11-S5c — `#t` (table length).
// SAFETY: `no_mangle` is required for Cranelift's `Linkage::Import` to resolve this symbol from the JIT'd code; this crate is the sole producer of `luna_jit_*` symbols.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_jit_table_len(t: i64) -> i64 {
    // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
    let vm = unsafe { current_jit_vm() };
    if vm.jit.pending_err.is_some() {
        return 0;
    }
    let g: luna_core::runtime::Gc<luna_core::runtime::Table> =
        luna_core::runtime::Gc::from_ptr(t as *mut luna_core::runtime::Table);
    // P11-S5d.E' — 5.4+ honours __len on tables; the helper bypasses it.
    // Park a deopt request and let the interpreter compute the length.
    if g.metatable().is_some() {
        vm.jit.pending_err = Some(vm.rt_err("JIT deopt: table has metatable"));
        return 0;
    }
    g.len()
}

// scoped_rebind submodule (formerly luna-jit/src/jit_backend/scoped_rebind.rs).
mod scoped_rebind;
