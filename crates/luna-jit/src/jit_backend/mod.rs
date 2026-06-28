//! P11 — JIT pipeline (luna crate side; the trait surface and pure
//! data types live in `luna_core::jit`).
//!
//! Closed sub-steps:
//!   - S0: cranelift substrate hookup (`2984c8b`).
//!   - S1: Proto → Cranelift IR lowerer for an int-arith subset (`9341c6c`).
//!   - S2: dispatch wire — `Vm::call_value` short-circuits to a cached
//!     native fn when the Proto fits the whitelist (`560bcfb`).
//!   - S2b: block-structured lowerer with conditional + unconditional
//!     branches. Whitelist gains `Jmp`, `Lt`, `Le`, `Eq` — a paired
//!     `Lt|Le|Eq` + `Jmp` is lowered as a cranelift `brif`.
//!
//! `try_compile_int_chunk` accepts a Proto when every opcode falls in
//! the cumulative whitelist; out-of-whitelist returns `None` and the
//! interpreter handles the chunk unchanged.

use cranelift::prelude::*;
use cranelift_codegen::ir::{BlockArg, UserFuncName};
use cranelift_frontend::FunctionBuilderContext;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use luna_core::jit::trace_types::{CompileOptions, CompiledTrace, TraceRecord};
use luna_core::jit::{
    CompileResult, IntChunkCompiler, IntChunkFn, IntFn1, IntFn2, IntFn3, IntFn4, JitVmGuard,
    MAX_JIT_ARITY, TraceCompiler,
};
use luna_core::runtime::Gc;
use luna_core::runtime::Value as LuaValue;
use luna_core::runtime::function::Proto;
use luna_core::vm::isa::{Inst, Op};

/// P11-S3 — per-Lua-register type lattice. `Unset` is the bottom;
/// `Int` and `Float` are incomparable monotypes. A register that's
/// pinned to both Int and Float in the same Proto causes the lowerer
/// to bail (`unify_kind` returns false). `Unset` registers that
/// stay Unset after the scan default to Int at emit time (they're
/// only used by Cranelift's SSA in unreachable tail slots).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RegKind {
    Unset,
    Int,
    Float,
    /// P11-S5c — `Gc<Table>` raw pointer pun. Represented as I64 at the
    /// Cranelift level (same shape as `RegKind::Int`) but kept distinct
    /// in the lattice so a register pinned to a table can't unify with
    /// one pinned to an integer; the whitelist gates on Table where it
    /// expects a table operand (e.g. `SetTable.A`, `Len.B`).
    Table,
}

impl RegKind {
    #[inline]
    fn unify(slot: &mut RegKind, incoming: RegKind) -> bool {
        match (*slot, incoming) {
            (_, RegKind::Unset) => true,
            (RegKind::Unset, _) => {
                *slot = incoming;
                true
            }
            (a, b) if a == b => true,
            // P11-S5d.C — Int+Table coexist (both I64-shaped at the
            // Cranelift level; `maybe_table[reg]` + Table-bail on
            // arith/cmp/ForPrep keeps the semantic guard).
            (RegKind::Int, RegKind::Table) | (RegKind::Table, RegKind::Int) => true,
            // P11-S5d.D step 3 — Float+Table coexist via I64↔F64
            // bitcast. The Variable is declared in whichever shape
            // the first writer pinned (F64 if Float first, I64 if
            // Table first); `aligned_def` handles the writer-side
            // bitcast and the emit-side `use_var` callers for
            // Table operands bitcast F64→I64 on the read side when
            // the declared slot is F64. Bit-pattern reinterpret is
            // lossless for an 8-byte pun (`f64::from_bits(ptr as
            // u64).to_bits()` round-trips exactly). Unlocks the
            // 5.1/5.2 `binary_trees` pattern: `if d == 0` uses
            // LoadF R[1]=0 (Float) in one BB, NewTable R[1]
            // (Table) in another — both safe per-BB, but our
            // pre-S5d.D `unify` rejected the slot reuse globally.
            (RegKind::Float, RegKind::Table) | (RegKind::Table, RegKind::Float) => true,
            _ => false,
        }
    }
}

// v1.1 A1 Session C — codegen-bearing modules live here on the luna
// side. `IntChunkFn`, the trait surface (`IntChunkCompiler`,
// `TraceCompiler`, `CompileResult`, `NullJitBackend`), `JitVmGuard`,
// and the pure trace data types moved to `luna_core::jit` so embedders
// who depend on luna-core alone never link Cranelift.
pub mod trace;

// v2.0 Track J sub-step J-A — `Send` wrapper newtype for
// `cranelift_jit::JITModule`. Pre-positioned for J-B's field
// migration of `JIT_CACHE` / `JIT_CACHE_HANDLES` /
// `TRACE_JIT_HANDLES` from `thread_local!` onto `Vm.VmJitStorage`.
// Scoped `pub(crate)` — no embedder surface.
mod send_jit_module;
#[allow(unused_imports)] // J-B will consume; J-A wires the wrapper only.
pub use send_jit_module::SendJitModule;

// v2.1 Phase 1K.D.1 — `JIT_VM` / `JIT_CL` TLS slots, the helper
// extern "C" fns, `enter_jit`, and `scoped_rebind` all moved to
// the sibling `luna-jit-helpers` crate so `luna-jit-llvm`
// (v2.1 alt backend) can reuse them without dragging Cranelift in.
// Star-re-export preserves every existing `super::luna_jit_*` /
// `crate::jit_backend::*` call path inside this crate.
pub use luna_jit_helpers::*;

// v2.0 Track J sub-step J-B — concrete per-`Vm` JIT storage struct
// (cache + cache_handles + trace_handles). Installed alongside the
// `CraneliftBackend` by `crate::install_default_jit`. luna-core sees
// it through the opaque `JitStorage` trait only.
pub(crate) mod storage;

// v1.1 A1 Session C — inline `#[cfg(test)] mod xx { ... }` blocks
// throughout this file call `crate::jit_backend::test_vm_new(version)` / `crate::jit_backend::test_vm_new_minimal(version)`
// and historically expected the Cranelift backend to be installed
// (v1.0 default). After the workspace split luna-core's `Vm::new`
// defaults to `NullJitBackend`, so we wrap construction in these
// helpers and replace the call sites by name.
#[cfg(test)]
fn test_vm_new(version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
    let mut vm = luna_core::vm::Vm::new(version);
    vm.install_jit_backend(CraneliftBackend, CraneliftBackend);
    // v2.0 Track J sub-step J-B — pair the backend install with the
    // CraneliftJitStorage so cache lookups can downcast.
    vm.install_jit_storage(storage::CraneliftJitStorage::default());
    vm
}
#[cfg(test)]
#[allow(dead_code)]
fn test_vm_new_minimal(version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
    let mut vm = luna_core::vm::Vm::new_minimal(version);
    vm.install_jit_backend(CraneliftBackend, CraneliftBackend);
    vm.install_jit_storage(storage::CraneliftJitStorage::default());
    vm
}

/// S4 — cross-`Vm` JIT cache. Look up the proto by a hash of its
/// bytecode + structural ABI fields; on miss, compile through
/// `try_compile_int_chunk` and store the result. Compiled mmap
/// pages live in the cache's `JITModule` so they outlast any single
/// `Vm`. Returns the 7-tuple `(entry_raw, num_args, returns_one,
/// arg_float_mask, arg_table_mask, ret_is_float, ret_is_table)` on
/// success (whether served from cache or freshly compiled), or
/// `None` when the proto's body falls outside the cumulative
/// whitelist.
///
/// S5a — `pre53` distinguishes dialects whose `ForPrep` / `ForLoop`
/// use the pre-5.3 `R[A] -= step + jmp` form (Lua 5.1 / 5.2 / 5.3)
/// from the 5.4+ count form (Lua 5.4 / 5.5). The same source loaded
/// in dialects on opposite sides of that split needs distinct
/// native code; this bit partitions the cache. For chunks that
/// don't touch `for` loops the bit is still hashed — same-source
/// 5.5 vs 5.5 still share; same-source 5.5 vs 5.1 don't.
///
/// S5d — adds `arg_table_mask` (per-arg `Gc<Table>` indicator) and
/// `ret_is_table` (true ↔ Return1 yields a `Gc<Table>` ptr).
pub fn cache_lookup_or_compile(
    storage: &mut dyn luna_core::jit::JitStorage,
    proto: luna_core::runtime::Gc<Proto>,
    pre53: bool,
    float_only: bool,
) -> Option<(*const u8, u8, bool, u8, u8, bool, bool)> {
    let key = proto_cache_key(&proto, pre53, float_only);
    // v2.0 Track J sub-step J-B Phase D — cache lookups read from the
    // per-`Vm` `storage.cache` field instead of the `JIT_CACHE` TLS.
    //
    // v2.0 J-B follow-up — `from_storage` returns `Result`; on
    // `StorageMismatch` (Vm.jit.storage isn't a CraneliftJitStorage)
    // skip JIT entirely. The dispatcher already treats `None` as
    // "this Proto stays on interp", so graceful skip = no JIT for
    // this Vm, no SIGABRT across any C-ABI boundary.
    let cs = storage::from_storage(storage).ok()?;
    let cached = cs.cache.get(&key).copied();
    if let Some(hit) = cached {
        return match hit {
            CacheEntry::Failed => None,
            CacheEntry::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            } => Some((
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            )),
        };
    }
    let entry = match try_compile_int_chunk(proto, pre53, float_only) {
        Some(handle) => {
            let raw = handle.entry_raw();
            let num_args = handle.num_args();
            let returns_one = handle.returns_one();
            let arg_float_mask = handle.arg_float_mask();
            let arg_table_mask = handle.arg_table_mask();
            let ret_is_float = handle.ret_is_float();
            let ret_is_table = handle.ret_is_table();
            // v2.0 Track J sub-step J-B Phase E — the JITModule the
            // handle owns holds the mmap. Park the handle on the
            // per-`Vm` storage so the entry_raw pointer stays valid
            // for the lifetime of this `Vm`. Append-only.
            //
            // v2.0 J-B follow-up — `from_storage` is `Result`-shaped
            // now. The `.ok()?` short-circuit above already verified
            // the storage was a `CraneliftJitStorage`, so on a sane
            // call this branch is unreachable. Guard with `match`
            // for honesty: on the impossible Err arm the compiled
            // `handle` drops (its `JITModule` releases the mmap) and
            // we return None — no leaked code page, no crash.
            match storage::from_storage(storage) {
                Ok(cs) => cs.cache_handles.push(handle),
                Err(_) => return None,
            }
            CacheEntry::Compiled {
                entry: raw,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            }
        }
        None => CacheEntry::Failed,
    };
    // v2.0 J-B follow-up — same `from_storage` is-Result rationale as
    // above; on the impossible Err branch we drop the freshly built
    // `entry` (it was `Copy`, no resource loss) and skip the cache
    // insert.
    storage::from_storage(storage)
        .ok()?
        .cache
        .insert(key, entry);
    match entry {
        CacheEntry::Failed => None,
        CacheEntry::Compiled {
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        } => Some((
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        )),
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CacheEntry {
    Failed,
    Compiled {
        entry: *const u8,
        num_args: u8,
        returns_one: bool,
        arg_float_mask: u8,
        arg_table_mask: u8,
        ret_is_float: bool,
        ret_is_table: bool,
    },
}

/// P11-S5d.J — classify every `Op::GetUpval` in `proto` as either the
/// existing **SelfMarker** role (the loaded value is used only as a
/// `Op::Call` func slot — S2c.C lowers that as a direct cranelift call
/// without ever materialising the upvalue) or the new **ValueRead**
/// role (the loaded value flows into arith / cmp / unary, so we need
/// the real value at runtime via `luna_jit_upval_get` and the pre53
/// path's "default Float" assumption is sound).
///
/// Algorithm: from each `GetUpval R[A]` at PC X, walk forward up to 8
/// instructions. The FIRST event for R[A] decides:
/// - `Op::Call` with `a == A` → SelfMarker
/// - arith / cmp / unary reading R[A] → ValueRead (the operand is
///   provably numeric: an interpreter would raise on a non-numeric
///   upvalue, so we won't miscompile silent data)
/// - any other op writing R[A] (or window end) → default SelfMarker,
///   which the rest of the scan handles via the existing arith-bail
///   on `self_upval`. Cases like `function () return x end` (Return1
///   reads R[A] without a numeric operator) stay in the default-bail
///   bucket because we can't assume the runtime type of `x`.
fn determine_getupval_roles(proto: &Proto) -> Vec<bool> {
    const WINDOW: usize = 8;
    let n = proto.code.len();
    let mut roles = vec![false; n];
    for pc in 0..n {
        let ins = proto.code[pc];
        if !matches!(ins.op(), Op::GetUpval) {
            continue;
        }
        let target_a = ins.a() as usize;
        let end = (pc + 1 + WINDOW).min(n);
        for q in (pc + 1)..end {
            let q_ins = proto.code[q];
            // Op::Call with R[A] as func slot — confirmed SelfMarker.
            if matches!(q_ins.op(), Op::Call) && q_ins.a() as usize == target_a {
                break;
            }
            // Arith / cmp / unary reading R[A] — confirmed ValueRead.
            if reads_register_a_arith(q_ins, target_a) {
                roles[pc] = true;
                break;
            }
            // R[A] overwritten before we confirmed either role — bail
            // to SelfMarker default (existing bail-on-arith-read in
            // the main scan handles it conservatively).
            if writes_register_a(q_ins, target_a) {
                break;
            }
        }
        // Window ended without an arith confirmation → roles[pc] stays
        // false (SelfMarker default).
    }
    roles
}

/// Helper for `determine_getupval_roles`: does `ins` *read* register
/// `target_a` via an arithmetic, comparison, or unary operator? These
/// are the ops whose interpreter semantics require a numeric operand
/// (otherwise PUC raises "attempt to perform arithmetic on a X
/// value"), so a JIT-side helper-fetch that interprets the upvalue
/// as Float is safe in pre53 dialects where Float is the only number
/// type.
fn reads_register_a_arith(ins: Inst, target_a: usize) -> bool {
    let b = ins.b() as usize;
    let c = ins.c() as usize;
    let a = ins.a() as usize;
    match ins.op() {
        Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Mod | Op::Pow | Op::IDiv => {
            b == target_a || c == target_a
        }
        Op::Lt | Op::Le | Op::Eq => a == target_a || b == target_a,
        Op::Unm | Op::BNot | Op::Not => b == target_a,
        _ => false,
    }
}

/// Helper for `determine_getupval_roles`: does `ins` write to register
/// index `target_a`? Conservative — list every op that has a write
/// target including range-writers (LoadNil, ForPrep, ForLoop) where
/// `target_a` may fall inside the affected range.
fn writes_register_a(ins: Inst, target_a: usize) -> bool {
    let a = ins.a() as usize;
    match ins.op() {
        Op::LoadI
        | Op::LoadF
        | Op::LoadK
        | Op::LoadKx
        | Op::LoadFalse
        | Op::LFalseSkip
        | Op::LoadTrue
        | Op::Move
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Mod
        | Op::Pow
        | Op::Div
        | Op::IDiv
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr
        | Op::Unm
        | Op::BNot
        | Op::Not
        | Op::Len
        | Op::Concat
        | Op::Call
        | Op::GetUpval
        | Op::GetTabUp
        | Op::GetTable
        | Op::GetI
        | Op::GetField
        | Op::NewTable
        | Op::SelfOp => a == target_a,
        Op::LoadNil => target_a >= a && target_a <= a + ins.b() as usize,
        Op::ForPrep | Op::ForLoop => target_a >= a && target_a <= a + 3,
        _ => false,
    }
}

/// S4 introspection (test-only): number of *Compiled* entries in
/// the given Vm's JIT cache (Failed cache slots are excluded so test
/// assertions over "compiled exactly once" don't drift when the
/// outer chunk's bail also occupies a slot).
///
/// v2.0 Track J sub-step J-B Phase D — takes `&Vm` since the cache
/// is now per-`Vm` (was thread-local). Pre-J-B was `#[cfg(test)]` —
/// lifted to pub so the J-B integration test (external binary, not
/// cfg(test) from this crate's POV) can probe per-`Vm` cache size
/// without a downcast. Harmless utility for any embedder.
pub fn cache_entry_count(vm: &luna_core::vm::Vm) -> usize {
    let storage = vm.jit.storage.as_ref().as_any();
    let cs = storage
        .downcast_ref::<storage::CraneliftJitStorage>()
        .expect("vm storage not CraneliftJitStorage");
    cs.cache
        .values()
        .filter(|e| matches!(e, CacheEntry::Compiled { .. }))
        .count()
}

/// S4 introspection (test-only): empty the Vm's JIT cache. Used
/// between tests that want to measure first-compile vs cache-hit
/// behaviour in isolation.
///
/// v2.0 Track J sub-step J-B Phase D — takes `&mut Vm` since the
/// cache is now per-`Vm` (was thread-local). Pre-J-B was
/// `#[cfg(test)]` — see [`cache_entry_count`] for the rationale.
pub fn cache_clear(vm: &mut luna_core::vm::Vm) {
    let storage = vm.jit.storage.as_mut().as_any_mut();
    if let Some(cs) = storage.downcast_mut::<storage::CraneliftJitStorage>() {
        cs.cache.clear();
        // v2.0 Track J sub-step J-B Phase E — also drop the cached
        // handles. Dropping each `JitHandle`'s `JITModule` releases
        // its mmap; tests that call `cache_clear` then re-eval can
        // observe the fresh compile.
        cs.cache_handles.clear();
    }
}

/// Stable cache key. The `proto.code` bytes + `num_params` +
/// `upvals.len()` + `max_stack` + every `consts[i]` that the lowerer
/// might read + the `pre53` dialect bit cover every input the
/// lowerer reads; two protos with identical bytecode AND identical
/// constants AND matching dialect share native code.
///
/// S3 added const hashing because two protos with identical
/// `LoadK k0 + Return1` shape but different `consts[0]` values
/// (e.g. `return 1+0.5` → Float(1.5) vs `return 0/0` → Float(NaN))
/// used to collide and the second chunk would return the first's
/// compiled constant.
///
/// S5a added the dialect bit: a `for i = 1, N do … end` chunk
/// compiles to a different shape in Lua 5.3 (pre-decrement + jmp
/// form) vs Lua 5.4/5.5 (count form). Mixing them in one cache
/// slot would either crash or compute the wrong sum.
fn proto_cache_key(proto: &Proto, pre53: bool, float_only: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for inst in proto.code.iter() {
        inst.0.hash(&mut h);
    }
    for c in proto.consts.iter() {
        match c {
            luna_core::runtime::Value::Int(i) => {
                0u8.hash(&mut h);
                i.hash(&mut h);
            }
            luna_core::runtime::Value::Float(f) => {
                1u8.hash(&mut h);
                f.to_bits().hash(&mut h);
            }
            // P11-S5b — string consts participate in the cache key via
            // their byte contents, not just their discriminant.
            // Two protos with identical bytecode but different
            // `GetField` k-operand strings (e.g. `math.sin` vs
            // `math.cos` — `GetField a=5 b=5 c=2` in both, but
            // `consts[2]` resolves to "sin" or "cos") would otherwise
            // collide and serve the wrong libm call from cache.
            luna_core::runtime::Value::Str(s) => {
                3u8.hash(&mut h);
                s.as_bytes().hash(&mut h);
            }
            // Other non-Int/Float consts still hash by discriminant so
            // unrelated protos stay distinct without paying for full
            // structural hashing of types we never inspect.
            other => {
                2u8.hash(&mut h);
                std::mem::discriminant(other).hash(&mut h);
            }
        }
    }
    proto.num_params.hash(&mut h);
    proto.upvals.len().hash(&mut h);
    proto.max_stack.hash(&mut h);
    pre53.hash(&mut h);
    float_only.hash(&mut h);
    h.finish()
}
// v1.1 A1 Session C — `IntFn1..4` + `MAX_JIT_ARITY` moved to
// `luna_core::jit` so `vm/exec.rs` (now in luna-core) can name them
// when transmuting JIT entry pointers. Bumping the arity cap stays
// mechanical: extend the alias list in `luna-core/src/jit/abi.rs`,
// add the matching match arm in `luna-core/src/vm/exec.rs`, then
// add the matching `IntFnN` codegen here.

/// P11-S5b — supported `math.<fn>(arg)` libm folds. Each entry is
/// the Lua-side method name (as it appears in `consts` after the
/// `GetField` k-operand) paired with the libm symbol the cranelift
/// `Linkage::Import` resolves to via `dlsym(RTLD_DEFAULT)`. Same
/// signature across all entries — `(f64) -> f64`. Single-arg
/// numerics only; `math.log(x, base)` / `math.atan(y, x)` /
/// `math.max(...)` use a different bytecode window (B≠2) so the
/// pattern matcher rejects them.
const MATH_LIBM_FNS: &[(&[u8], &str)] = &[
    (b"sin", "sin"),
    (b"cos", "cos"),
    (b"tan", "tan"),
    (b"asin", "asin"),
    (b"acos", "acos"),
    (b"atan", "atan"),
    (b"exp", "exp"),
    (b"log", "log"),
    (b"sqrt", "sqrt"),
    (b"floor", "floor"),
    (b"ceil", "ceil"),
];

/// P11-S5c.C — `Table` layout constants used by the inline-aset
/// fast path. Cranelift IR walks past the helper call ABI by
/// loading the table's array pointer and length directly from the
/// `Gc<Table>` raw ptr, skipping the per-iter thread-local read
/// + `Gc::from_ptr` non-null check + `Table::set_int` dispatch
/// that the helper-call path pays.
///
/// The offsets are computed at compile time via `std::mem::offset_of!`,
/// so the IR follows whatever `Table`'s `#[repr(C)]` layout chooses
/// today. The static asserts below pin the assumptions the IR
/// itself can't verify (`Box<[u64]>` as a `(ptr, len)` fat pointer,
/// `RawVal` packed to 8 bytes, and the `Table.asize` field width).
///
/// P11-S5d.H/I — Table now keeps `array_ptr: *mut u8` as the single
/// source of truth for "where does the array part live?". The pointer
/// targets either the inline storage embedded in the Table struct
/// (asize <= INLINE_ASIZE) or an external `slab: Box<[u64]>`. The JIT
/// loads `array_ptr` directly — no branching, no `slab.ptr` indirection
/// — and computes `atags_ptr = array_ptr + asize * 8` on the fly.
pub(crate) const TABLE_ARRAY_PTR_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, array_ptr);
pub(crate) const TABLE_ASIZE_OFFSET: usize = std::mem::offset_of!(luna_core::runtime::Table, asize);
/// P11-S5d.K — `Option<Gc<Table>>` is 8 bytes via NPO; 0 ⇔ None.
/// Inline aget reads this to short-circuit on metatable.is_none()
/// rather than always going through the helper's metatable check.
pub(crate) const TABLE_METATABLE_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, metatable);

/// v2.1 Phase 1I.B — table-field IC scaffold.
///
/// Byte offset of the `nodes: Box<[Node]>` field's low fat-pointer
/// word (the data pointer). luna-core's `runtime::table::jit_layout`
/// module computes this against the live `Table` struct, then we
/// re-export it here so trace.rs can refer to it locally.
///
/// Fat-pointer layout: `(data_ptr, len)` — the data ptr is at
/// `TABLE_NODES_PTR_OFFSET`, length at `TABLE_NODES_LEN_OFFSET`
/// (= `..PTR_OFFSET + 8`). See
/// `runtime/table.rs::phase_1i_b_node_layout_pinned` for the runtime
/// assertion that pins this ABI.
#[allow(dead_code)]
pub(crate) const TABLE_NODES_PTR_OFFSET: usize =
    luna_core::runtime::table::jit_layout::TABLE_NODES_OFFSET;
/// High fat-pointer word — the `len` (in `Node` slots) of the
/// `nodes: Box<[Node]>`. The IC's shape-stability guard compares
/// this load against the recorder's cached length.
#[allow(dead_code)]
pub(crate) const TABLE_NODES_LEN_OFFSET: usize = TABLE_NODES_PTR_OFFSET + 8;
/// Within one `Node`, the byte offset of `key: Value`. Value's tag
/// byte (`#[repr(C, u8)]`) lives at offset 0 of the Value, so the
/// key's tag is at `NODE_KEY_OFFSET` (= 0) and the key's raw
/// 8-byte payload at `NODE_KEY_OFFSET + 8`.
#[allow(dead_code)]
pub(crate) const NODE_KEY_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_KEY_OFFSET;
/// Byte offset of `val: Value` within a `Node`. The val's tag is
/// at `NODE_VAL_OFFSET` (= 16), payload at `NODE_VAL_OFFSET + 8`.
#[allow(dead_code)]
pub(crate) const NODE_VAL_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_VAL_OFFSET;
/// Total `Node` size in bytes — stride for `node_addr = nodes_ptr +
/// slot_idx * SIZEOF_NODE`.
#[allow(dead_code)]
pub(crate) const SIZEOF_NODE: usize = luna_core::runtime::table::jit_layout::SIZEOF_NODE;
/// Byte offset of the value's tag byte inside the `val: Value` field
/// of a `Node`. Value is `#[repr(C, u8)]`, discriminant at byte 0.
#[allow(dead_code)]
pub(crate) const NODE_VAL_TAG_OFFSET: usize = NODE_VAL_OFFSET;
/// Byte offset of the value's 8-byte raw payload inside `val: Value`.
/// 7 bytes of alignment padding sit between the tag and the payload.
#[allow(dead_code)]
pub(crate) const NODE_VAL_RAW_OFFSET: usize = NODE_VAL_OFFSET + 8;
/// Byte offset of the key's 8-byte raw payload inside `key: Value`.
/// IC's "slot key still matches" guard reads 8 bytes here and
/// compares against the recorder-cached `Gc<LuaStr>` pointer bits.
#[allow(dead_code)]
pub(crate) const NODE_KEY_RAW_OFFSET: usize = NODE_KEY_OFFSET + 8;
/// Byte offset of the key's tag byte (`#[repr(C, u8)]`). The IC
/// also guards `key.tag == raw::STR` so a recycled slot that happens
/// to hold a non-string key with matching raw bits would deopt.
#[allow(dead_code)]
pub(crate) const NODE_KEY_TAG_OFFSET: usize = NODE_KEY_OFFSET;

const RAW_TAG_INT: i64 = luna_core::runtime::value::raw::INT as i64;
const RAW_TAG_FLOAT: i64 = luna_core::runtime::value::raw::FLOAT as i64;
const RAW_TAG_TABLE: i64 = luna_core::runtime::value::raw::TABLE as i64;
const RAW_TAG_NIL: i64 = luna_core::runtime::value::raw::NIL as i64;

const _: () = {
    assert!(std::mem::size_of::<*mut u8>() == 8);
    assert!(std::mem::size_of::<luna_core::runtime::value::RawVal>() == 8);
    assert!(std::mem::align_of::<luna_core::runtime::value::RawVal>() == 8);
    // P11-S5d.H — `asize` is u64 so a single `load i64` yields the
    // array-part length; the JIT then shifts left 3 to multiply by 8
    // for the `atags_ptr = array_ptr + asize * 8` computation.
    assert!(std::mem::size_of::<u64>() == 8);
};

/// P11-S5b — a single recognized `math.<fn>(arg)` fold. The four
/// participating PCs are `start_pc + 0..=3` (GetTabUp / GetField /
/// Move / Call). At emit time only the `GetTabUp` PC produces IR —
/// the other three are no-ops and the outer pc cursor jumps past
/// them.
#[derive(Clone, Copy)]
struct MathFold {
    /// PC of the `GetTabUp` that opens the fold.
    start_pc: usize,
    /// libm symbol name. Static — points into `MATH_LIBM_FNS`.
    fn_name: &'static str,
    /// Lua register holding the argument (the `Move`'s source).
    arg_reg: u32,
    /// Lua register receiving the libm result (= the `GetTabUp.A` =
    /// `Call.A`).
    dst_reg: u32,
}

/// v1.3 Phase AOT Stage 3 — backend-agnostic metadata describing one
/// lowered Lua chunk's ABI shape. Returned by [`lower_int_chunk_into`]
/// so callers (runtime JIT today, ahead-of-time `luna-aot` tomorrow)
/// can wrap the produced [`FuncId`] in their own dispatch handle.
#[derive(Clone, Copy, Debug)]
pub struct ChunkMeta {
    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    pub num_args: u8,
    /// True when the Lua chunk this fn was lowered from contains a
    /// `Return1`; false when only `Return0` is present.
    pub returns_one: bool,
    /// Bit `i = 1` ↔ arg slot `i` is f64 (passed as i64 bit-pattern).
    pub arg_float_mask: u8,
    /// Bit `i = 1` ↔ arg slot `i` is `Gc<Table>` raw ptr.
    pub arg_table_mask: u8,
    /// True iff the Proto's `Return1` value is f64.
    pub ret_is_float: bool,
    /// True iff the Proto's `Return1` value is a `Gc<Table>` raw ptr.
    pub ret_is_table: bool,
}

/// v1.3 Phase AOT Stage 3 — build a fresh `JITModule` configured with
/// all `luna_jit_*` helper symbols pre-registered. Shared by the
/// runtime JIT entry [`try_compile_int_chunk`] and tests; the AOT
/// pipeline (luna-aot) builds an `ObjectModule` instead and feeds it
/// to the same [`lower_int_chunk_into`] generic body.
fn build_jit_module_with_helpers() -> Option<JITModule> {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").ok();
    flag_builder.set("is_pic", "false").ok();
    flag_builder.set("opt_level", "speed").ok();
    let isa = cranelift_native::builder()
        .ok()?
        .finish(settings::Flags::new(flag_builder))
        .ok()?;
    let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    // P11-S5c — register Rust helper symbols so the cranelift JIT can
    // resolve them at finalize time. Without this, executables that
    // link luna as an rlib strip the `#[no_mangle]` symbols at link
    // time and the default `dlsym(RTLD_DEFAULT)` resolver fails. The
    // libm symbols S5b uses (`sin`, `cos`, …) are linked from libc
    // and stay resolvable via dlsym, so they don't need this step.
    builder.symbol("luna_jit_new_table", luna_jit_new_table as *const u8);
    builder.symbol(
        "luna_jit_new_table_sized",
        luna_jit_new_table_sized as *const u8,
    );
    builder.symbol(
        "luna_jit_table_set_int",
        luna_jit_table_set_int as *const u8,
    );
    builder.symbol(
        "luna_jit_table_set_float_float",
        luna_jit_table_set_float_float as *const u8,
    );
    builder.symbol(
        "luna_jit_table_get_int",
        luna_jit_table_get_int as *const u8,
    );
    builder.symbol(
        "luna_jit_table_get_float",
        luna_jit_table_get_float as *const u8,
    );
    builder.symbol("luna_jit_table_len", luna_jit_table_len as *const u8);
    builder.symbol("luna_jit_upval_get", luna_jit_upval_get as *const u8);
    Some(JITModule::new(builder))
}

/// Try to JIT-compile `proto`. Returns `None` when any opcode in the
/// body falls outside the cumulative whitelist — the interpreter then
/// handles the chunk unchanged. `pre53` (Lua 5.1 / 5.2 / 5.3) selects
/// the pre-5.3 `ForPrep` / `ForLoop` form and currently makes S5a's
/// loop lowering bail; pass `false` (Lua 5.4 / 5.5) to enable the
/// counted-loop emit. The dialect bit also participates in the
/// thread-local cache key — see `proto_cache_key`.
///
/// v1.3 Phase AOT Stage 3 — thin wrapper around the backend-agnostic
/// [`lower_int_chunk_into`] generic; constructs a `JITModule`,
/// finalizes the compiled fn into RWX memory, and wraps the entry ptr
/// in a [`JitHandle`] that owns the module for the entry's lifetime.
pub fn try_compile_int_chunk(proto: Gc<Proto>, pre53: bool, float_only: bool) -> Option<JitHandle> {
    let mut module = build_jit_module_with_helpers()?;
    let (fn_id, meta) = lower_int_chunk_into(&mut module, proto, pre53, float_only)?;
    module.finalize_definitions().ok()?;

    // P11-S5d.C diag — `LUNA_JIT_TRACE=1` prints one line per
    // successful JIT compile with the Proto's source location +
    // signature. Future S5d.C work hitting a regression in
    // (e.g.) errors.lua can grep this trace to pinpoint the
    // exact `load(...)` snippet that JIT'd, instead of bisecting
    // by hand. The check is one TLS read per compile when the
    // env var is unset — negligible vs the cranelift codegen
    // cost.
    if std::env::var_os("LUNA_JIT_TRACE").is_some() {
        let src_bytes = proto.source.as_bytes();
        let src = std::str::from_utf8(src_bytes).unwrap_or("<non-utf8 source>");
        let line_start = proto.line_defined;
        let line_end = proto.last_line_defined;
        let ChunkMeta {
            num_args,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
            ..
        } = meta;
        eprintln!(
            "[luna jit] {src}:{line_start}-{line_end} params={} code_len={} num_args={num_args} arg_float_mask={arg_float_mask:#x} arg_table_mask={arg_table_mask:#x} ret_is_float={ret_is_float} ret_is_table={ret_is_table}",
            proto.num_params,
            proto.code.len(),
        );
    }

    let ptr = module.get_finalized_function(fn_id);
    Some(JitHandle {
        // v2.0 Track J sub-step J-D — wrap with the `SendJitModule`
        // sleeve. SAFETY criterion (default `SystemMemoryProvider`) is
        // satisfied by `build_jit_module_with_helpers` which never
        // calls `JITBuilder::memory_provider`; see send_jit_module.rs.
        _module: SendJitModule::new(module),
        entry_raw: ptr,
        num_args: meta.num_args,
        returns_one: meta.returns_one,
        arg_float_mask: meta.arg_float_mask,
        arg_table_mask: meta.arg_table_mask,
        ret_is_float: meta.ret_is_float,
        ret_is_table: meta.ret_is_table,
    })
}

/// v1.3 Phase AOT Stage 3 — backend-agnostic body of the int-chunk
/// lowerer. Generic over any `cranelift_module::Module` so the same
/// codegen pipeline drives the runtime JIT (`JITModule`,
/// [`try_compile_int_chunk`]) and the AOT pipeline (`ObjectModule` in
/// `luna-aot`).
///
/// Returns `None` when any opcode in the body falls outside the
/// cumulative whitelist (same gate as [`try_compile_int_chunk`]). On
/// success returns the declared [`FuncId`] for the lowered chunk
/// alongside ABI metadata; the caller drives backend-specific
/// finalization (`JITModule::finalize_definitions` /
/// `ObjectModule::finish`).
pub fn lower_int_chunk_into<M: Module>(
    module: &mut M,
    proto: Gc<Proto>,
    pre53: bool,
    float_only: bool,
) -> Option<(FuncId, ChunkMeta)> {
    if proto.num_params > MAX_JIT_ARITY {
        return None;
    }
    let num_params = proto.num_params as usize;
    // S2c.C — luna's `local function f(...) end` idiom binds upvalue 0
    // (Lua 5.5/5.4/5.3/5.2) or upvalue 1 (Lua 5.1 — slot 0 is the
    // `_ENV` placeholder) to the closure itself. S3 generalises the
    // upvalue tracking: the scanner watches GetUpval(b) and pins the
    // self-upval index from the first occurrence; subsequent
    // GetUpval(b') with b' != self-upval-idx bails. Upvals count is
    // bounded only to avoid pathological cases.
    let allows_self_recursion = !proto.upvals.is_empty() && proto.upvals.len() <= 4;
    let mut self_upval_idx: Option<u32> = None;
    // First pass: verify every op is supported AND scan for basic-block
    // boundaries. A BB starts at PC 0, at every jump target, and at the
    // instruction immediately after a terminator (Jmp, Return, or a
    // paired Lt|Le|Eq+Jmp).
    let n = proto.code.len();
    let mut bb_starts = vec![false; n];
    if n == 0 {
        return None;
    }
    bb_starts[0] = true;
    let mut sees_return1 = false;
    // Per-register "this slot last held a self-upval-loaded closure"
    // tag. Carried across Move; cleared by any other writer. Lookup
    // at Op::Call decides whether it's a self-recursive call we can
    // lower. Indexed by Lua register number.
    let max_stack = (proto.max_stack as usize).max(num_params);
    let mut self_upval: Vec<bool> = vec![false; max_stack];
    // P11-S5d.J — per-PC role for `Op::GetUpval`. SelfMarker (true at
    // the bool position is misleading — see the enum-like split below)
    // is the original S2c.C behavior; ValueRead enables fetching the
    // upvalue value at runtime via `luna_jit_upval_get` so chunks like
    // `function () return k * k end` can JIT. `is_upval_value_read[pc]`
    // is true iff the role is ValueRead. Pre-pass below decides via
    // an 8-op lookahead from each `GetUpval`.
    let is_upval_value_read: Vec<bool> = determine_getupval_roles(&proto);
    // PC of every Op::Call that resolves to the self-recursion edge.
    // Emit-side consumes this to lower as a cranelift `call fn_id`.
    let mut self_call_pcs: Vec<bool> = vec![false; n];
    // S5a — track each register's last-written `LoadI` immediate (or
    // None when it was overwritten by anything else). `ForPrep` reads
    // `step_const[A+2]` to check that the step is a compile-time
    // constant ≠ 0 — non-immediate steps bail to the interpreter.
    let mut step_const: Vec<Option<i64>> = vec![None; max_stack];
    // S5a — every JIT'd ForPrep/ForLoop pair, in source order. Each
    // tuple is `(prep_pc, loop_pc, step_imm)`. Emit consumes this to
    // lay out the counted-loop blocks.
    let mut for_loops: Vec<(usize, usize, i64)> = Vec::new();

    // P11-S5c — `defines_table[reg]` tracks whether a `NewTable` or
    // `Move` from a defined table reg has run by the current scan
    // position. Reset on any non-table-producing write to the
    // register. SetTable / GetI / Len require the operand to be
    // marked.
    //
    // Limitation: this is a single forward pass without BB-level
    // intersection at join points. To stay correct in the presence
    // of conditional branching, we bail any chunk that has both a
    // `NewTable` AND any conditional op (`Lt` / `Le` / `Eq`) — see
    // the `has_conditional` / `has_new_table` end check below.
    // Without that restriction a conditional NewTable would be
    // represented at the SetTable use site by a cranelift phi node
    // merging the table ptr with the entry-block iconst(0), and
    // the false-branch path would feed NULL into the Rust helper.
    let mut defines_table: Vec<bool> = vec![false; max_stack];
    // P11-S5d.A — function params are guaranteed defined by the
    // caller. The dispatcher's `try_jit_call_op` only marshals
    // `Value::Table` into a Table-typed slot (via `arg_table_mask`),
    // so a Table-typed param truly holds a valid `Gc<Table>` ptr at
    // entry. Treat all params as table-defined upfront; the RegKind
    // sweep still rejects a non-Table param being used as a table
    // (the kind mismatch surfaces there as a unify failure).
    for i in 0..num_params {
        if let Some(slot) = defines_table.get_mut(i) {
            *slot = true;
        }
    }

    // P11-S5c.B — per-PC presize hint for `Op::NewTable`. When a
    // NewTable is immediately followed by the canonical
    // `LoadI init / LoadI/LoadK limit / LoadI step / ForPrep`
    // window with `init = 1`, `step = 1`, `limit = N` (Int const),
    // emit reaches for `luna_jit_new_table_sized(N)` to skip the
    // table-fill loop's intermediate rehashes. Map is sparse —
    // only NewTables that match the pattern get an entry. Filled
    // by a second scan pass below (the main whitelist pass already
    // produces `for_loops`, which gives us the matching ForPrep
    // PCs cheaply).
    let mut presize_for_newtable: std::collections::HashMap<usize, i64> =
        std::collections::HashMap::new();

    // P11-S5b — pre-scan: detect `math.<fn>(arg)` 4-op folds. The
    // pattern is dialect-invariant — Lua 5.1 through 5.5 all emit
    // the same `GetTabUp / GetField / Move / Call` window for
    // `<env>.math.<fn>(<reg>)`. When a window matches, every
    // participating PC is marked `folded_math[pc] = true` so the
    // main whitelist loop below accepts them in-place; emit folds
    // them into a single cranelift libm call.
    //
    // Requires: `proto.upvals[0].name == "_ENV"`. luna's frontend
    // always parks the env upvalue at slot 0 (5.5/5.4/5.3/5.2: the
    // sole upvalue of any chunk; 5.1: an explicit `_ENV` placeholder
    // even though 5.1 source has no lexical `_ENV`). Any other shape
    // bails the fold for that PC.
    let mut folded_math: Vec<bool> = vec![false; n];
    let mut math_folds: Vec<MathFold> = Vec::new();
    let env_upval_present = proto
        .upvals
        .first()
        .map(|u| &*u.name == "_ENV")
        .unwrap_or(false);
    if env_upval_present {
        let mut try_pc = 0usize;
        while try_pc + 3 < n {
            if let Some(fold) = try_match_math_fold(&proto, try_pc) {
                folded_math[try_pc] = true;
                folded_math[try_pc + 1] = true;
                folded_math[try_pc + 2] = true;
                folded_math[try_pc + 3] = true;
                math_folds.push(fold);
                try_pc += 4;
            } else {
                try_pc += 1;
            }
        }
    }

    let mut pc = 0;
    while pc < n {
        let ins = proto.code[pc];
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
                // S3 — Float constants pass. S5a — Int constants also
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
                    // S5a — a `LoadK Int(v)` also pins the register
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
                // P11-S5d.G — `R[A..=A+B] = nil`. The whitelist accepts
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
                // P11-S5c — propagate table-defined-ness through
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
                // Bail out — S2c.C only handles the call-target case.
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
            Op::GetUpval => {
                let b = ins.b();
                if (b as usize) >= proto.upvals.len() {
                    return None;
                }
                // P11-S5d.J — ValueRead role: `R[A]` is consumed as
                // a real value (not a self-recursion call target).
                // For now we restrict to **Float-only dialects**
                // (5.1/5.2) so we can default-pin the upvalue's
                // runtime type to Float without a tag check. 5.3+
                // has Int subtype — the upvalue could be Int at
                // runtime; a Float interpretation would garble the
                // raw bits. 5.3+ would need a tag check + deopt path;
                // deferred. `pre53 && float_only` ⇔ 5.1 or 5.2.
                if is_upval_value_read[pc] {
                    if !float_only {
                        return None;
                    }
                    // Don't tag `self_upval` — this GetUpval feeds an
                    // arith/cmp/Return reader. Clear ancillary trackers
                    // mirror-style at R[A].
                    if let Some(slot) = step_const.get_mut(ins.a() as usize) {
                        *slot = None;
                    }
                    if let Some(slot) = defines_table.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                    pc += 1;
                    continue;
                }
                // SelfMarker — existing S2c.C behavior.
                if !allows_self_recursion {
                    return None;
                }
                // Pin self-upval idx on first GetUpval; reject any
                // subsequent GetUpval that reads a different slot.
                // This is dialect-agnostic — Lua 5.5/5.4/5.3/5.2 fib
                // reads upvals[0], Lua 5.1 fib reads upvals[1] (with
                // upvals[0] being an unused `_ENV` placeholder).
                match self_upval_idx {
                    Some(idx) if idx != b => return None,
                    Some(_) => {}
                    None => self_upval_idx = Some(b),
                }
                if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                    *slot = true;
                }
                if let Some(slot) = step_const.get_mut(ins.a() as usize) {
                    *slot = None;
                }
            }
            Op::Call => {
                let a = ins.a() as usize;
                // nargs / nresults bounds (apply to both self-recursive
                // and math-fold variants — `MathFold` already pins B=2
                // C=2, well within MAX_JIT_ARITY).
                //
                let nargs = ins.b().checked_sub(1)?;
                let c = ins.c();
                // P11-S5d.C — variadic Call (C=0) paired with a
                // variadic SetList (B=0) at PC+1 is the
                // `{make(d-1), make(d-1)}` shape — luna's frontend
                // emits the second sibling's `Call` as variadic so
                // it can splat into the next SetList. Every JIT'd
                // chunk has `returns_one == true`, so the variadic
                // count is statically 1 and the SetList's implied
                // length is `A_call - A_list` (computed on the
                // SetList side).
                let next_is_variadic_setlist = c == 0
                    && pc + 1 < n
                    && matches!(proto.code[pc + 1].op(), Op::SetList)
                    && proto.code[pc + 1].b() == 0;
                let nresults = if next_is_variadic_setlist {
                    1
                } else {
                    c.checked_sub(1)?
                };
                if nargs > MAX_JIT_ARITY as u32 || nresults != 1 {
                    return None;
                }
                if folded_math[pc] {
                    // P11-S5b — math libcall fold. Emit-side folds the
                    // 4-op window into one cranelift libm call; here
                    // we just clear the per-register trackers.
                } else if self_upval.get(a).copied().unwrap_or(false) {
                    // S2c.C — self-recursive call.
                    self_call_pcs[pc] = true;
                } else {
                    return None;
                }
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(a) {
                    *slot = None;
                }
            }
            Op::GetTabUp | Op::GetField => {
                // P11-S5b — accepted only as part of a recognized
                // math libcall fold. The fold's emit consumes all
                // four PCs; the per-register trackers for R[A] get
                // cleared so post-fold uses see fresh state.
                if !folded_math[pc] {
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
            Op::Return1 => {
                // A Return1 of a self-upval-tagged register would return
                // the (mismarked) closure value back to the caller — not
                // a generic shape S2c.C handles. Bail.
                if self_upval.get(ins.a() as usize).copied().unwrap_or(false) {
                    return None;
                }
                sees_return1 = true;
                if pc + 1 < n {
                    bb_starts[pc + 1] = true;
                }
            }
            Op::Return0 => {
                if pc + 1 < n {
                    bb_starts[pc + 1] = true;
                }
            }
            Op::Jmp => {
                let tgt = jmp_target(pc, ins);
                if tgt >= n {
                    return None;
                }
                bb_starts[tgt] = true;
                if pc + 1 < n {
                    bb_starts[pc + 1] = true;
                }
            }
            Op::Lt | Op::Le | Op::Eq => {
                // Reading a tagged register here is a generic-upvalue
                // comparison (e.g. `if n_upval < 3 then …`) which S2c.C
                // doesn't model.
                if self_upval.get(ins.a() as usize).copied().unwrap_or(false)
                    || self_upval.get(ins.b() as usize).copied().unwrap_or(false)
                {
                    return None;
                }
                // A comparison op is always paired with a following Jmp
                // (PUC's `cond_skip` invariant). luna's compiler never
                // emits one without the other; if we see a lone Lt/Le/Eq
                // the proto is malformed for our purposes — bail out.
                let &jmp = proto.code.get(pc + 1)?;
                if !matches!(jmp.op(), Op::Jmp) {
                    return None;
                }
                let jmp_pc = pc + 1;
                let tgt = jmp_target(jmp_pc, jmp);
                if tgt >= n {
                    return None;
                }
                bb_starts[tgt] = true;
                if jmp_pc + 1 < n {
                    bb_starts[jmp_pc + 1] = true;
                }
                pc = jmp_pc; // outer pc += 1 below moves past the Jmp
            }
            Op::ForPrep => {
                // S5a + S5a.B — both forms admitted. The dialect-
                // specific shape is picked up in emit, gated by `pre53`.
                let a = ins.a() as usize;
                // The step has to be a compile-time-known `LoadI`
                // immediate. luna's bytecode emitter always materialises
                // numeric-for steps via a `LoadI` (`for i = 1, N do …` →
                // step register pre-loaded with `LoadI 1`). A non-Int
                // step (`for i = 1, N, x` where x is a variable) bails.
                let step_imm = step_const.get(a + 2).copied().flatten()?;
                if step_imm == 0 {
                    return None;
                }
                // Pair with the matching ForLoop. luna's interpreter
                // executes `add_pc(bx - 1)` *after* the natural
                // `pc += 1` post-step, so the running pc lands on the
                // OP_FORLOOP at `prep_pc + bx`. See
                // `src/vm/exec.rs::for_prep` (post53 branch).
                let loop_pc = pc + ins.bx() as usize;
                if loop_pc >= n {
                    return None;
                }
                let loop_ins = proto.code[loop_pc];
                if !matches!(loop_ins.op(), Op::ForLoop) || loop_ins.a() as usize != a {
                    return None;
                }
                // BB boundaries: ForPrep is its own block; body starts
                // at pc+1; the ForLoop sits in the body's tail block;
                // the exit lands at loop_pc+1.
                bb_starts[pc + 1] = true;
                if loop_pc + 1 < n {
                    bb_starts[loop_pc + 1] = true;
                }
                bb_starts[loop_pc] = true; // ForLoop opens its own block.
                for_loops.push((pc, loop_pc, step_imm));
                // ForPrep writes R[A], R[A+1], R[A+2], R[A+3] — every
                // register's step_const tracker is stale after this.
                for off in 0..=3 {
                    if let Some(slot) = step_const.get_mut(a + off) {
                        *slot = None;
                    }
                    if let Some(slot) = self_upval.get_mut(a + off) {
                        *slot = false;
                    }
                }
            }
            Op::ForLoop => {
                // ForLoop alone (without a paired ForPrep earlier in
                // the for_loops list) is an orphan — luna's bytecode
                // emitter never produces that, so reject any ForLoop
                // whose matching ForPrep wasn't recorded.
                let a = ins.a() as usize;
                if !for_loops.iter().any(|&(_, lp, _)| lp == pc) {
                    return None;
                }
                // ForLoop writes R[A], R[A+1], R[A+3] on the continue
                // path — same step_const wipe as ForPrep.
                for off in [0usize, 1, 3] {
                    if let Some(slot) = step_const.get_mut(a + off) {
                        *slot = None;
                    }
                    if let Some(slot) = self_upval.get_mut(a + off) {
                        *slot = false;
                    }
                }
            }
            Op::NewTable => {
                // P11-S5c — empty-table form. luna's frontend emits
                // NewTable a=A b=0 c=0 for `{}`.
                // P11-S5d.B — also accept `b > 0` (array presize for
                // `{...}` literals); the emit-side calls
                // `luna_jit_new_table_sized(b)`. `c > 0` (hash part
                // presize) still bails — none of our headline cells
                // use hash literals, and the per-slot lowering would
                // need a separate dispatch for `nodes`.
                if ins.c() != 0 {
                    return None;
                }
                let a = ins.a() as usize;
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(a) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(a) {
                    *slot = true;
                }
            }
            Op::SetTable => {
                // P11-S5c — register-keyed set. The proper safety
                // gate (R[A] must be a definitively-defined table at
                // this PC) lives in the BB-level dataflow check
                // below; the linear `defines_table` walk would
                // wrongly accept a false-branch-only NewTable.
            }
            Op::SetList => {
                // P11-S5d.B — fixed-count array literal initializer
                // (B > 0). P11-S5d.C — variadic form (B == 0, C ==
                // 0) accepted when paired with the immediately
                // preceding `Op::Call C=0`; the JIT'd self-recursive
                // callee returns exactly 1 value, so the static
                // count is `A_call - A_list`.
                let b = ins.b();
                if ins.c() != 0 {
                    return None;
                }
                if b == 0 {
                    if pc == 0 {
                        return None;
                    }
                    let prev = proto.code[pc - 1];
                    if !matches!(prev.op(), Op::Call) || prev.c() != 0 {
                        return None;
                    }
                    let a_call = prev.a() as i64;
                    let a_list = ins.a() as i64;
                    if a_call <= a_list {
                        return None;
                    }
                }
                // BB-level dataflow verifies R[A] is a table at this
                // PC. No register-tracker side effects — SetList
                // writes through R[A] into the table's array part,
                // not into R[A..A+B] themselves.
            }
            Op::GetI => {
                // P11-S5c — `R[A] = R[B][imm(C)]`. BB-level dataflow
                // verifies R[B] is a table at this PC.
                let a = ins.a() as usize;
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(a) {
                    *slot = None;
                }
                // R[A] receives an Int value pulled from the table;
                // it is not itself a table reference.
                if let Some(slot) = defines_table.get_mut(a) {
                    *slot = false;
                }
            }
            Op::GetTable => {
                // P11-S5d.E' — `R[A] = R[B][R[C]]`. BB-level dataflow
                // verifies R[B] is a table at this PC. Parallel to
                // GetI but the key is in a register (5.1/5.2 lower
                // `t[1]` this way because they have no Int subtype:
                // the literal `1` lands in a register via `LoadF 1.0`
                // and then `OP_GETTABLE` reads it).
                let a = ins.a() as usize;
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(a) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(a) {
                    *slot = false;
                }
            }
            Op::Len => {
                // P11-S5c — `R[A] = #R[B]`. BB-level dataflow
                // verifies R[B] is a table at this PC.
                let a = ins.a() as usize;
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
                if let Some(slot) = step_const.get_mut(a) {
                    *slot = None;
                }
                if let Some(slot) = defines_table.get_mut(a) {
                    *slot = false;
                }
            }
            _ => return None,
        }
        pc += 1;
    }

    // P11-S5d.B — BB-level dataflow for "is this register a
    // table at the use site". Replaces S5c's
    // `has_new_table && has_conditional → bail` blanket safety
    // net: that gate was sound but rejected the make-style
    // pattern where both branches of an Op::Eq + Jmp split
    // independently `NewTable R[A]` and then SetList into it.
    //
    // Forward dataflow:
    //   entry[BB] = intersection of exit[pred] for each predecessor
    //   exit[BB] = apply ops in BB body forward from entry[BB]
    //   entry[BB 0] = function params marked true (caller guarantee)
    //
    // After convergence we re-walk every PC; at each
    // SetTable / SetList / GetI / Len / Move-from-table, derive
    // the local state from `entry[bb]` + body-apply up to PC and
    // verify the relevant register is in the table-defined set.
    //
    // Move propagation is included so e.g. `local t = {}` (R[0])
    // followed by `Move R[5] = R[0]` and then SetTable R[5][...]
    // works — the existing `table_alloc_10k` pattern.
    let bb_pcs: Vec<usize> = (0..n)
        .filter(|&p| bb_starts.get(p).copied().unwrap_or(false))
        .collect();
    let num_bbs = bb_pcs.len();
    if num_bbs == 0 {
        return None;
    }
    let mut pc_to_bb: Vec<usize> = vec![0; n];
    for (idx, &start) in bb_pcs.iter().enumerate() {
        let end = bb_pcs.get(idx + 1).copied().unwrap_or(n);
        for p in start..end {
            pc_to_bb[p] = idx;
        }
    }

    // Build successors per BB via op-level semantics. Returns
    // (terminator-found, successor-bb-indices).
    let mut bb_successors: Vec<Vec<usize>> = vec![Vec::new(); num_bbs];
    for bb_idx in 0..num_bbs {
        let bb_start = bb_pcs[bb_idx];
        let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
        let mut found_terminator = false;
        let mut p = bb_start;
        while p < bb_end {
            let ins = proto.code[p];
            match ins.op() {
                Op::Jmp => {
                    let tgt = jmp_target(p, ins);
                    if tgt < n {
                        let s = pc_to_bb[tgt];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::Lt | Op::Le | Op::Eq => {
                    // Paired with the next op (always Jmp per scan).
                    let jmp = proto.code[p + 1];
                    let tgt = jmp_target(p + 1, jmp);
                    if tgt < n {
                        let s = pc_to_bb[tgt];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    let fall = p + 2;
                    if fall < n {
                        let s = pc_to_bb[fall];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::Return0 | Op::Return1 => {
                    found_terminator = true;
                    break;
                }
                Op::ForPrep => {
                    let fall = p + 1;
                    if fall < n {
                        let s = pc_to_bb[fall];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    if let Some(&(_, lp, _)) = for_loops.iter().find(|&&(pp, _, _)| pp == p) {
                        let exit_pc = lp + 1;
                        if exit_pc < n {
                            let s = pc_to_bb[exit_pc];
                            if !bb_successors[bb_idx].contains(&s) {
                                bb_successors[bb_idx].push(s);
                            }
                        }
                    }
                    found_terminator = true;
                    break;
                }
                Op::ForLoop => {
                    let exit_pc = p + 1;
                    if exit_pc < n {
                        let s = pc_to_bb[exit_pc];
                        if !bb_successors[bb_idx].contains(&s) {
                            bb_successors[bb_idx].push(s);
                        }
                    }
                    if let Some(&(prep, _, _)) = for_loops.iter().find(|&&(_, lp, _)| lp == p) {
                        let body = prep + 1;
                        if body < n {
                            let s = pc_to_bb[body];
                            if !bb_successors[bb_idx].contains(&s) {
                                bb_successors[bb_idx].push(s);
                            }
                        }
                    }
                    found_terminator = true;
                    break;
                }
                _ => {
                    p += 1;
                }
            }
        }
        if !found_terminator && bb_end < n {
            let s = pc_to_bb[bb_end];
            if !bb_successors[bb_idx].contains(&s) {
                bb_successors[bb_idx].push(s);
            }
        }
    }

    let mut bb_predecessors: Vec<Vec<usize>> = vec![Vec::new(); num_bbs];
    for src in 0..num_bbs {
        for &dst in &bb_successors[src] {
            if !bb_predecessors[dst].contains(&src) {
                bb_predecessors[dst].push(src);
            }
        }
    }

    // Body-apply: forward semantics for one BB's body, mutating `state`.
    let body_apply = |bb_idx: usize, state: &mut Vec<bool>| {
        let bb_start = bb_pcs[bb_idx];
        let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
        for p in bb_start..bb_end {
            let ins = proto.code[p];
            match ins.op() {
                Op::NewTable => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = true;
                    }
                }
                Op::Move => {
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    let src_def = state.get(b).copied().unwrap_or(false);
                    if let Some(slot) = state.get_mut(a) {
                        *slot = src_def;
                    }
                }
                Op::GetI | Op::GetTable | Op::Len => {
                    // Result is Int — not a table.
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                }
                Op::LoadI | Op::LoadF | Op::LoadK | Op::Add | Op::Sub | Op::Mul | Op::Div => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                }
                Op::LoadNil => {
                    // P11-S5d.G — LoadNil writes Nil to R[A..=A+B];
                    // none of those are table refs.
                    let a = ins.a() as usize;
                    for off in 0..=(ins.b() as usize) {
                        if let Some(slot) = state.get_mut(a + off) {
                            *slot = false;
                        }
                    }
                }
                Op::Call => {
                    // Self-recursive (the only Call shape the scan
                    // admits outside the math fold) may return a
                    // table when `ret_kind` is Table — but the kind
                    // sweep that decides that hasn't run yet at this
                    // point in the pass. Treat conservatively: clear
                    // the bit. RegKind sweep + emit will catch any
                    // mismatch as a unify failure / IR-time bail.
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                }
                Op::GetUpval | Op::GetTabUp | Op::GetField => {
                    // None of these produce a table-valued result in
                    // the current whitelist (math fold's GetTabUp /
                    // GetField are consumed in-line).
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                }
                Op::ForPrep | Op::ForLoop => {
                    let a = ins.a() as usize;
                    for off in 0..=3 {
                        if let Some(slot) = state.get_mut(a + off) {
                            *slot = false;
                        }
                    }
                }
                // SetTable / SetList write *through* R[A]; the table
                // ref itself stays whatever it was.
                _ => {}
            }
        }
    };

    // P11-S5d.B — "must-defined" dataflow uses intersection at
    // joins, so we initialise non-entry BBs at the TOP element
    // (every register considered defined) and refine downward.
    // Starting at BOTTOM (false) would make the intersection at
    // any back-edge converge to false immediately.
    let mut bb_entry: Vec<Vec<bool>> = (0..num_bbs).map(|i| vec![i != 0; max_stack]).collect();
    let mut bb_exit: Vec<Vec<bool>> = vec![vec![true; max_stack]; num_bbs];
    // Entry BB starts with params marked as defined (caller guarantee
    // mirrors the linear walk's init above).
    for i in 0..max_stack {
        if let Some(slot) = bb_entry[0].get_mut(i) {
            *slot = i < num_params;
        }
    }
    let mut iters = 0;
    let max_iters = num_bbs * (max_stack + 2);
    let mut changed = true;
    while changed && iters < max_iters {
        changed = false;
        iters += 1;
        for bb_idx in 0..num_bbs {
            let new_entry = if bb_predecessors[bb_idx].is_empty() {
                // Unreachable BB or BB 0. Keep existing entry (params
                // marked at start for BB 0; all-false for others).
                bb_entry[bb_idx].clone()
            } else {
                let mut e = bb_exit[bb_predecessors[bb_idx][0]].clone();
                for &pred in &bb_predecessors[bb_idx][1..] {
                    for (i, val) in bb_exit[pred].iter().enumerate() {
                        e[i] &= val;
                    }
                }
                if bb_idx == 0 {
                    for i in 0..num_params {
                        if let Some(slot) = e.get_mut(i) {
                            *slot = true;
                        }
                    }
                }
                e
            };
            let mut state = new_entry.clone();
            body_apply(bb_idx, &mut state);
            if state != bb_exit[bb_idx] {
                bb_exit[bb_idx] = state;
                changed = true;
            }
            if new_entry != bb_entry[bb_idx] {
                bb_entry[bb_idx] = new_entry;
                changed = true;
            }
        }
    }

    // Per-use BB-level safety check.
    for p in 0..n {
        let ins = proto.code[p];
        let check_reg = match ins.op() {
            Op::SetTable | Op::SetList => Some(ins.a() as usize),
            Op::GetI | Op::GetTable | Op::Len => Some(ins.b() as usize),
            _ => None,
        };
        if let Some(reg) = check_reg {
            let bb_idx = pc_to_bb[p];
            let bb_start = bb_pcs[bb_idx];
            let mut state = bb_entry[bb_idx].clone();
            // Apply ops up to (but not including) p.
            for q in bb_start..p {
                let prev = proto.code[q];
                match prev.op() {
                    Op::NewTable => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = true;
                        }
                    }
                    Op::Move => {
                        let a = prev.a() as usize;
                        let b = prev.b() as usize;
                        let src_def = state.get(b).copied().unwrap_or(false);
                        if let Some(slot) = state.get_mut(a) {
                            *slot = src_def;
                        }
                    }
                    Op::GetI | Op::GetTable | Op::Len => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::LoadI | Op::LoadF | Op::LoadK | Op::Add | Op::Sub | Op::Mul | Op::Div => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::LoadNil => {
                        let a = prev.a() as usize;
                        for off in 0..=(prev.b() as usize) {
                            if let Some(slot) = state.get_mut(a + off) {
                                *slot = false;
                            }
                        }
                    }
                    Op::Call => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::GetUpval | Op::GetTabUp | Op::GetField => {
                        if let Some(slot) = state.get_mut(prev.a() as usize) {
                            *slot = false;
                        }
                    }
                    Op::ForPrep | Op::ForLoop => {
                        let a = prev.a() as usize;
                        for off in 0..=3 {
                            if let Some(slot) = state.get_mut(a + off) {
                                *slot = false;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !state.get(reg).copied().unwrap_or(false) {
                return None;
            }
        }
    }

    // P11-S5c.B — find every NewTable that opens a
    // `NewTable R[A]=`{}`; LoadI R[A+1]=1; LoadI|LoadK R[A+2]=N;
    // LoadI R[A+3]=1; ForPrep R[A+1]` window. The matching ForPrep
    // is already in `for_loops`; we walk that list and look 4 PCs
    // back. Sizing hint = N (the `limit` const, at the third op of
    // the window). Bench source `for i = 1, 10000 do t[i] = i end`
    // matches; arbitrary loop bodies after ForPrep don't affect the
    // pattern (we only inspect the four ops between NewTable and
    // ForPrep, inclusive).
    for &(prep_pc, _, step_imm) in &for_loops {
        if step_imm != 1 || prep_pc < 4 {
            continue;
        }
        let nt_pc = prep_pc - 4;
        let init_pc = prep_pc - 3;
        let limit_pc = prep_pc - 2;
        let step_pc = prep_pc - 1;

        let nt = proto.code[nt_pc];
        let init = proto.code[init_pc];
        let limit = proto.code[limit_pc];
        let step = proto.code[step_pc];
        let fp = proto.code[prep_pc];

        if !matches!(nt.op(), Op::NewTable) {
            continue;
        }
        if nt.b() != 0 || nt.c() != 0 {
            continue;
        }
        let fp_base = fp.a() as i64;
        if (nt.a() as i64) + 1 != fp_base {
            continue;
        }
        // R[A+1] = init = LoadI 1.
        if !matches!(init.op(), Op::LoadI) || init.a() as i64 != fp_base || init.sbx() != 1 {
            continue;
        }
        // R[A+2] = limit = LoadI or LoadK Int. sbx fits in i32; we
        // already clamp at the helper.
        let limit_val: i64 = match limit.op() {
            Op::LoadI if limit.a() as i64 == fp_base + 1 => limit.sbx() as i64,
            Op::LoadK if limit.a() as i64 == fp_base + 1 => {
                let bx = limit.bx() as usize;
                match proto.consts.get(bx).copied() {
                    Some(LuaValue::Int(v)) => v,
                    _ => continue,
                }
            }
            _ => continue,
        };
        // R[A+3] = step = LoadI 1.
        if !matches!(step.op(), Op::LoadI) || step.a() as i64 != fp_base + 2 || step.sbx() != 1 {
            continue;
        }
        if limit_val <= 0 || limit_val > (1 << 27) {
            continue;
        }
        presize_for_newtable.insert(nt_pc, limit_val);
    }

    // P11-S5b — every math fold's internal PCs (+1, +2, +3) must
    // sit inside a single basic block. A Jmp target landing on one
    // of them would leave a half-emitted fold straddling a Cranelift
    // block boundary (the BB algorithm marks the target as a block
    // start but emit's `pc += 3` jumps over it without visiting).
    // luna's frontend never produces such a jump, but bail
    // defensively to keep the IR well-formed.
    for fold in &math_folds {
        for off in 1..=3 {
            if bb_starts.get(fold.start_pc + off).copied().unwrap_or(false) {
                return None;
            }
        }
    }

    // S2c.C correctness gate: every JIT-recognised self-recursive call
    // bypasses luna's `c_depth` / `frames.len()` budget. A self-call
    // with no base case before it would blow the OS stack (the
    // `runtime_stack_overflow_is_caught` regression). Require at least
    // one Return reachable from PC 0 WITHOUT passing through a self-
    // recursive Call PC. fib has the early `if n < 2 then return n end`
    // path; `f() return 1 + f() end` has no such path and bails.
    let any_self_call = self_call_pcs.iter().any(|&b| b);
    if any_self_call {
        let mut visited = vec![false; n];
        let mut stack = vec![0usize];
        visited[0] = true;
        let mut safe_return_reached = false;
        while let Some(pc) = stack.pop() {
            let ins = proto.code[pc];
            match ins.op() {
                Op::Return0 | Op::Return1 => {
                    safe_return_reached = true;
                    break;
                }
                Op::Jmp => {
                    let tgt = jmp_target(pc, ins);
                    if tgt < n && !visited[tgt] {
                        visited[tgt] = true;
                        stack.push(tgt);
                    }
                }
                Op::Lt | Op::Le | Op::Eq => {
                    // skip the paired Jmp's PC; consider both successors
                    let jmp = proto.code[pc + 1];
                    let jmp_tgt = jmp_target(pc + 1, jmp);
                    if jmp_tgt < n && !visited[jmp_tgt] {
                        visited[jmp_tgt] = true;
                        stack.push(jmp_tgt);
                    }
                    let fall = pc + 2;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                }
                Op::Call if self_call_pcs[pc] => {
                    // self-recursive — treat as a wall; do NOT traverse past.
                }
                Op::ForPrep => {
                    // Two successors: fall-through (body) AND the
                    // paired ForLoop's exit (skip when empty). Either
                    // path can reach a Return.
                    let fall = pc + 1;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                    if let Some(&(_, lp, _)) = for_loops.iter().find(|&&(p, _, _)| p == pc) {
                        let exit = lp + 1;
                        if exit < n && !visited[exit] {
                            visited[exit] = true;
                            stack.push(exit);
                        }
                    }
                }
                _ => {
                    let fall = pc + 1;
                    if fall < n && !visited[fall] {
                        visited[fall] = true;
                        stack.push(fall);
                    }
                }
            }
        }
        if !safe_return_reached {
            return None;
        }
    }

    // S3 — per-register type inference. Each Lua register holds either
    // an Int (i64) or a Float (f64). A register that's pinned to both
    // shapes within the same Proto bails the lowerer. The sweep is
    // forward-only with a fixpoint loop because a self-recursive Call
    // result kind depends on the Proto's own return kind (carried via
    // `ret_kind`); successive passes propagate the resolved kind.
    let mut reg_kinds: Vec<RegKind> = vec![RegKind::Unset; max_stack];
    let mut ret_kind: RegKind = RegKind::Unset;
    // P11-S5d.C scaffolding (unused until per-BB kind tracking lands)
    // — `latest_writer_kind[reg]` records the kind written to `reg`
    // by the most recent writer op in linear PC order during this
    // sweep pass. With the current per-proto `RegKind` model
    // (strict Int/Table conflict), this tracker is a no-op: every
    // op that writes a Variable's kind also passes through the
    // global unify, so latest_writer_kind never disagrees with
    // reg_kinds. The scaffold is wired in so a future S5d.C can
    // relax `unify` (e.g. Int + Table → joint) and the Return1 ret
    // kind can be picked from the latest writer rather than the
    // joint kind. See `make_proto_5_5_round_trip` (currently still
    // bails) for the motivating shape.
    let mut latest_writer_kind: Vec<RegKind>;
    // P11-S5d.C — `maybe_table[reg]` is set when the register
    // could hold a Table pointer at runtime even though
    // `reg_kinds[reg]` says Int. The classic case is
    // `Op::GetI R[A] = R[B][c]`: the helper returns the raw
    // payload bits regardless of the stored Value's tag, so
    // when the slot held a Table at runtime R[A] is a Gc<Table>
    // pun. Downstream arith / Lt-Le / ForPrep use this tag to
    // bail conservatively (interp would have raised; the JIT'd
    // `iadd` / `icmp` would silently compute garbage).
    let mut maybe_table: Vec<bool>;
    // P11-S5d.G — parallel to `maybe_table`: this register's most
    // recent writer was `Op::LoadNil`, so a kind-sensitive reader
    // (arith, cmp, SetTable's helper) would silently read `Int(0)`
    // where the Lua semantics demand a Nil error or Nil tag. SetList
    // emit consumes Nil-tagged stores via `current_is_nil` and so
    // does NOT bail on a Nil source; arith/cmp/SetTable scan bail.
    let mut is_nil_writer: Vec<bool>;
    for _ in 0..4 {
        let pre_regs = reg_kinds.clone();
        let pre_ret = ret_kind;
        latest_writer_kind = vec![RegKind::Unset; max_stack];
        maybe_table = vec![false; max_stack];
        // Function args (R[0..num_params]) carry valid Values from the
        // caller. Locals (R[num_params..max_stack]) come in as Nil from
        // the interp's frame-init clear. PUC 5.1 optimizes away the
        // LoadNil for declared-uninitialized locals at function start
        // (`luaK_nil` suppresses if pc==0 + reg above nactvar). Without
        // pre-marking those as nil_writer, JIT arith reading an
        // uninitialized 5.1 local would silently consume the cranelift
        // Variable's default 0 instead of raising "arithmetic on nil".
        // See docs/known-bugs/fixed/jit-uninitialized-local-arith.md
        // (filed 2026-06-22 by tests/e2e_programs.rs::err_arith_on_nil).
        is_nil_writer = vec![false; max_stack];
        for r in num_params..max_stack {
            is_nil_writer[r] = true;
        }
        let mut pc = 0;
        while pc < n {
            let ins = proto.code[pc];
            match ins.op() {
                Op::LoadI => {
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Int) {
                        return None;
                    }
                    latest_writer_kind[ins.a() as usize] = RegKind::Int;
                    maybe_table[ins.a() as usize] = false;
                    is_nil_writer[ins.a() as usize] = false;
                }
                Op::LoadF => {
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Float) {
                        return None;
                    }
                    latest_writer_kind[ins.a() as usize] = RegKind::Float;
                    maybe_table[ins.a() as usize] = false;
                    is_nil_writer[ins.a() as usize] = false;
                }
                Op::LoadK => {
                    // Whitelist guarantees Int or Float const.
                    let bx = ins.bx() as usize;
                    let kind = match proto.consts[bx] {
                        LuaValue::Float(_) => RegKind::Float,
                        LuaValue::Int(_) => RegKind::Int,
                        _ => unreachable!("whitelist gates non-numeric consts"),
                    };
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], kind) {
                        return None;
                    }
                    latest_writer_kind[ins.a() as usize] = kind;
                    maybe_table[ins.a() as usize] = false;
                    is_nil_writer[ins.a() as usize] = false;
                }
                Op::LoadNil => {
                    // P11-S5d.G — `R[A..=A+B] = nil`. Leave `reg_kinds`
                    // alone so a downstream writer (e.g. 5.1/5.2's
                    // `LoadF R[3] = 1.0` after an earlier
                    // `LoadNil R[3]` in the same Proto) can pin its
                    // own kind without a unify conflict. The 8-byte
                    // payload of Nil is 0, which is a lossless bit
                    // pattern under either I64 or F64 Variable
                    // (`aligned_def` bitcasts at the write site).
                    // SetList emit overrides to `RAW_TAG_NIL` via the
                    // BB-local `current_is_nil` shadow. The
                    // `is_nil_writer` sweep tracker propagates through
                    // Move and bails any arith/cmp/SetTable/Return1
                    // reader so e.g. `nil + 1` or `t[nil] = 1` or
                    // `return nil` falls through to the interpreter
                    // (which raises the correct error or returns Nil).
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    for off in 0..=b {
                        let r = a + off;
                        // `latest_writer_kind` left untouched: a
                        // subsequent reader that bypassed the
                        // `is_nil_writer` bail would land on the prior
                        // writer's kind, which is correct.
                        maybe_table[r] = false;
                        is_nil_writer[r] = true;
                    }
                }
                Op::Move => {
                    // P11-S5b — fold-internal Move (slot +2 of a math
                    // libcall) writes a temp register the libm emit
                    // never reads (the emit pulls the arg straight
                    // from `fold.arg_reg`). The temp gets clobbered
                    // by the next opcode — either by the same fold's
                    // Call result, or by a subsequent fold's
                    // GetTabUp. Forcing a kind here just creates a
                    // false conflict.
                    if folded_math[pc] {
                        // pc advances normally at the loop's tail.
                    } else {
                        let src_kind = reg_kinds[ins.b() as usize];
                        if !RegKind::unify(&mut reg_kinds[ins.a() as usize], src_kind) {
                            return None;
                        }
                        let lwk = latest_writer_kind[ins.b() as usize];
                        latest_writer_kind[ins.a() as usize] = lwk;
                        maybe_table[ins.a() as usize] = maybe_table[ins.b() as usize];
                        is_nil_writer[ins.a() as usize] = is_nil_writer[ins.b() as usize];
                    }
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div => {
                    let b = ins.b() as usize;
                    let c = ins.c() as usize;
                    // P11-S5d.C — Table operand makes Lua's interp
                    // error ("attempt to perform arithmetic on a
                    // table value") while the JIT's `iadd` would
                    // happily compute on ptr bits. Check
                    // `reg_kinds`, `latest_writer_kind`,
                    // and `maybe_table` (GetI returns whose
                    // payload could be a stored Table).
                    if matches!(reg_kinds[b], RegKind::Table)
                        || matches!(reg_kinds[c], RegKind::Table)
                        || matches!(latest_writer_kind[b], RegKind::Table)
                        || matches!(latest_writer_kind[c], RegKind::Table)
                        || maybe_table[b]
                        || maybe_table[c]
                    {
                        return None;
                    }
                    // P11-S5d.G — `nil + x` / `x + nil` raises in interp
                    // (`attempt to perform arithmetic on a nil value`);
                    // the JIT would silently `iadd(0, x)`. Bail so the
                    // interpreter surfaces the error.
                    if is_nil_writer[b] || is_nil_writer[c] {
                        return None;
                    }
                    let kb = reg_kinds[b];
                    let kc = reg_kinds[c];
                    if !RegKind::unify(&mut reg_kinds[b], kc) {
                        return None;
                    }
                    if !RegKind::unify(&mut reg_kinds[c], kb) {
                        return None;
                    }
                    let merged = reg_kinds[b];
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], merged) {
                        return None;
                    }
                    // Op::Div is Float-only in PUC 5.5 semantics
                    // (integer `/` always coerces to float). Pin to
                    // Float here so a chunk like `local x = a / b`
                    // where a/b are Unset still resolves.
                    if matches!(ins.op(), Op::Div)
                        && !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Float)
                    {
                        return None;
                    }
                    // Arith result is Int or Float — never Table —
                    // so clear the maybe_table tag on R[A].
                    maybe_table[ins.a() as usize] = false;
                    is_nil_writer[ins.a() as usize] = false;
                    // Arith result kind picked from the operands'
                    // local kinds: any Float → Float (PUC's mixed
                    // promotion semantic); else Int.
                    let lwk_b = latest_writer_kind[b];
                    let lwk_c = latest_writer_kind[c];
                    let arith_kind = if matches!(lwk_b, RegKind::Float)
                        || matches!(lwk_c, RegKind::Float)
                        || matches!(ins.op(), Op::Div)
                    {
                        RegKind::Float
                    } else {
                        RegKind::Int
                    };
                    latest_writer_kind[ins.a() as usize] = arith_kind;
                }
                Op::Lt | Op::Le | Op::Eq => {
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    // P11-S5d.C — Lt/Le errors on a Table; Eq is
                    // semantically safe (Lua's Eq across types is
                    // always false, and our icmp on ptr bits
                    // matches that for typical addresses).
                    if matches!(ins.op(), Op::Lt | Op::Le)
                        && (matches!(reg_kinds[a], RegKind::Table)
                            || matches!(reg_kinds[b], RegKind::Table)
                            || matches!(latest_writer_kind[a], RegKind::Table)
                            || matches!(latest_writer_kind[b], RegKind::Table)
                            || maybe_table[a]
                            || maybe_table[b])
                    {
                        return None;
                    }
                    // P11-S5d.G — `nil < x` / `nil <= x` raise; `nil == x`
                    // is well-defined in Lua but our icmp would compare
                    // raw 0 bits ≠ proper Nil tag and miss the nil-aware
                    // path. Bail conservatively.
                    if is_nil_writer[a] || is_nil_writer[b] {
                        return None;
                    }
                    let ka = reg_kinds[a];
                    let kb = reg_kinds[b];
                    if !RegKind::unify(&mut reg_kinds[a], kb) {
                        return None;
                    }
                    if !RegKind::unify(&mut reg_kinds[b], ka) {
                        return None;
                    }
                }
                Op::GetUpval => {
                    // S3 — no kind constraint for the SelfMarker role.
                    // The self-upval marker is never read as a real
                    // value (the matching Op::Call rewrites to a
                    // direct cranelift call, bypassing the register).
                    // The Variable's declared type is decided by
                    // whatever else reads R[A] around this — typically
                    // a later same-register arith result whose kind we
                    // already pinned. The emit-side `aligned_def`
                    // makes the placeholder zero match whatever
                    // declared type we picked.
                    //
                    // 5.2 fib hits this: R[1] is LoadF'd to Float, then
                    // re-used by GetUpval(self), then Call writes the
                    // Float self-result. Pinning Int here would conflict
                    // with the LoadF and bail the whole Proto.
                    //
                    // P11-S5d.J — ValueRead role: pin R[A] to Float so
                    // downstream arith picks `fadd`/`fmul`. Restricted
                    // to pre53 (linear pre-pass already bails non-pre53
                    // value-read).
                    if is_upval_value_read[pc] {
                        let a = ins.a() as usize;
                        if !RegKind::unify(&mut reg_kinds[a], RegKind::Float) {
                            return None;
                        }
                        latest_writer_kind[a] = RegKind::Float;
                        maybe_table[a] = false;
                        is_nil_writer[a] = false;
                    }
                }
                Op::Call => {
                    if folded_math[pc] {
                        // P11-S5b — math libcall result is f64. Pin R[A]
                        // (= Call.A = libm return slot) to Float.
                        if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Float) {
                            return None;
                        }
                        latest_writer_kind[ins.a() as usize] = RegKind::Float;
                        maybe_table[ins.a() as usize] = false;
                        is_nil_writer[ins.a() as usize] = false;
                    } else {
                        // Self-recursive call result kind = the Proto's
                        // own ret kind.
                        if !RegKind::unify(&mut reg_kinds[ins.a() as usize], ret_kind) {
                            return None;
                        }
                        if !matches!(ret_kind, RegKind::Unset) {
                            latest_writer_kind[ins.a() as usize] = ret_kind;
                        }
                        // The self-recursive callee's return kind is
                        // statically known; clear any prior
                        // maybe_table tag on R[A].
                        maybe_table[ins.a() as usize] = false;
                        is_nil_writer[ins.a() as usize] = false;
                    }
                }
                Op::GetTabUp | Op::GetField => {
                    // P11-S5b — folded GetTabUp / GetField don't ever
                    // observe their stored values (the next fold op
                    // overwrites R[A]). The Call PC pins R[A] to
                    // Float on its own; nothing to do here.
                    if !folded_math[pc] {
                        return None;
                    }
                }
                Op::Return1 => {
                    // P11-S5d.G — Return1 on a LoadNil-written register
                    // would wrap `Int(0)` instead of `Nil` (the helper
                    // ABI is i64 bits; the dispatcher uses ret_kind to
                    // decide Int vs Float, not Nil). Bail to interp so
                    // a `function () return nil end` returns Nil, not
                    // Int(0).
                    if is_nil_writer[ins.a() as usize] {
                        return None;
                    }
                    // P11-S5d.C — pick from the most recent writer
                    // instead of the unified `reg_kinds` slot so a
                    // `LoadI 0 → Eq → NewTable → Return1` chain
                    // sees the Return as a Table return (not Int).
                    let a_kind = latest_writer_kind[ins.a() as usize];
                    let a_kind = if matches!(a_kind, RegKind::Unset) {
                        reg_kinds[ins.a() as usize]
                    } else {
                        a_kind
                    };
                    if !RegKind::unify(&mut ret_kind, a_kind) {
                        return None;
                    }
                    // Late: now that ret_kind may have been pinned,
                    // back-propagate to R[A] so a Float ret pins the
                    // register's type even when R[A] was Unset.
                    //
                    // P11-S5d.E' — guard on Unset: a 5.1/5.2
                    // `LoadF + GetTable + Return1` chain reuses R[A]
                    // as the Float-key holder before GetTable stores
                    // the raw-payload result. `reg_kinds[a]` already
                    // pinned Float by LoadF; we set `ret_kind = Int`
                    // (the helper's raw-payload contract, latest
                    // writer = Int). Unifying Float vs Int here would
                    // bail the chunk needlessly — the Variable stays
                    // F64, and the Return1 emit bitcasts the F64 use
                    // back to I64 so the i64 bits ferry through.
                    if matches!(reg_kinds[ins.a() as usize], RegKind::Unset)
                        && !RegKind::unify(&mut reg_kinds[ins.a() as usize], ret_kind)
                    {
                        return None;
                    }
                }
                Op::Return0 | Op::Jmp => {}
                Op::ForPrep | Op::ForLoop => {
                    // S5a / S5a.B — Int loop. S5a.C — Float loop (5.1 /
                    // 5.2 numeric `for` keeps the loop var Float). The
                    // loop kind is decided by R[A]'s scanned kind: Float
                    // at any pass forces Float for R[A], R[A+1], R[A+3]
                    // (Unset / Int → Int path, the existing behaviour).
                    // R[A+2] (step) is independent: PUC's numeric-for
                    // compiler always emits an Int step immediate (LoadI
                    // 1 / -1 / …), even in 5.1 / 5.2 Float loops, so we
                    // pin it Int regardless and the Float emit promotes
                    // the immediate to f64const at use sites.
                    //
                    // P11-S5d.C — with the relaxed Int+Table `unify`
                    // a `for i = 1, {}, 10 do … end` chunk's `limit`
                    // slot (R[A+1]) holds a Table while `reg_kinds`
                    // says Int. The interpreter raises "for limit
                    // must be a number"; the JIT's `isub(ptr, 1)` /
                    // `icmp` would silently compute a junk count and
                    // exit cleanly, returning success where Lua
                    // would have raised. Reject any of the four
                    // loop slots being Table at the latest write,
                    // including `maybe_table` (a GetI return).
                    let a = ins.a() as usize;
                    let loop_kind = match reg_kinds[a] {
                        RegKind::Float => RegKind::Float,
                        RegKind::Int | RegKind::Unset => RegKind::Int,
                        RegKind::Table => return None,
                    };
                    for off in [0usize, 1, 2, 3] {
                        if matches!(latest_writer_kind[a + off], RegKind::Table)
                            || maybe_table[a + off]
                        {
                            return None;
                        }
                    }
                    for off in [0usize, 1, 3] {
                        if !RegKind::unify(&mut reg_kinds[a + off], loop_kind) {
                            return None;
                        }
                        latest_writer_kind[a + off] = loop_kind;
                        maybe_table[a + off] = false;
                        is_nil_writer[a + off] = false;
                    }
                    if !RegKind::unify(&mut reg_kinds[a + 2], RegKind::Int) {
                        return None;
                    }
                    latest_writer_kind[a + 2] = RegKind::Int;
                    maybe_table[a + 2] = false;
                    is_nil_writer[a + 2] = false;
                }
                Op::NewTable => {
                    // S5c — R[A] = fresh empty table.
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Table) {
                        return None;
                    }
                    latest_writer_kind[ins.a() as usize] = RegKind::Table;
                    // A freshly-NewTable'd register isn't a
                    // maybe-Int — it's definitely a Table — so
                    // clear the maybe_table tag too. arith etc.
                    // already bail via the RegKind::Table check.
                    maybe_table[ins.a() as usize] = false;
                    is_nil_writer[ins.a() as usize] = false;
                }
                Op::SetList => {
                    // P11-S5d.B — `R[A][1..=B] = R[A+1..A+B]`. R[A]
                    // must be Table; the per-element kinds (Int /
                    // Float / Table / Nil) are inspected at emit time
                    // (current_kinds + current_is_nil) so we tag-store
                    // correctly. No kind constraint pushed onto
                    // R[A+i] here — let upstream writers pin them.
                    if !RegKind::unify(&mut reg_kinds[ins.a() as usize], RegKind::Table) {
                        return None;
                    }
                }
                Op::SetTable => {
                    // S5c — R[A] (table) Table. Key/value pair must
                    // be either (Int, Int) or (Float, Float). Mixed
                    // shapes (Int key + Float value) aren't required
                    // by any current bench source — luna's frontend
                    // emits `Move + Move + Move + SetTable` where
                    // the Moves come from the same loop var (so the
                    // pair shares kind). Bail mixed shapes.
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    let c = ins.c() as usize;
                    if !RegKind::unify(&mut reg_kinds[a], RegKind::Table) {
                        return None;
                    }
                    // P11-S5d.G — `t[nil] = x` raises in interp (
                    // "table index is nil"); the JIT's Int helper
                    // would silently set `t[0] = x`. `t[k] = nil`
                    // would write `Int(0)` instead of removing the
                    // entry. Either Nil operand bails to interp.
                    if is_nil_writer[b] || is_nil_writer[c] {
                        return None;
                    }
                    // Key and value must unify with each other —
                    // they're typically two Moves of the same source.
                    let kb = reg_kinds[b];
                    let kc = reg_kinds[c];
                    if !RegKind::unify(&mut reg_kinds[b], kc) {
                        return None;
                    }
                    if !RegKind::unify(&mut reg_kinds[c], kb) {
                        return None;
                    }
                    // Pin them to Int by default if still Unset; the
                    // Float branch is reachable only when one side
                    // was already Float-pinned by a prior op (e.g. a
                    // LoadF or a Float-typed loop var).
                    let resolved = reg_kinds[b];
                    if matches!(resolved, RegKind::Unset) {
                        if !RegKind::unify(&mut reg_kinds[b], RegKind::Int) {
                            return None;
                        }
                        if !RegKind::unify(&mut reg_kinds[c], RegKind::Int) {
                            return None;
                        }
                    } else if !matches!(resolved, RegKind::Int | RegKind::Float) {
                        // Table-typed key/value — out of scope.
                        return None;
                    }
                }
                Op::GetI => {
                    // S5c — R[A] = R[B][imm(C)]. R[B] must be Table;
                    // R[A] is Int (matches the static Int-only store
                    // expectation of `luna_jit_table_get_int`).
                    // P11-S5d.C — the helper returns raw payload
                    // bits regardless of the slot's actual Value
                    // tag; if the table stored a Table at that
                    // index the read value is a Gc<Table> pun.
                    // Mark R[A] maybe_table so subsequent arith /
                    // Lt-Le / ForPrep bail conservatively.
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                        return None;
                    }
                    if !RegKind::unify(&mut reg_kinds[a], RegKind::Int) {
                        return None;
                    }
                    maybe_table[a] = true;
                    is_nil_writer[a] = false;
                }
                Op::GetTable => {
                    // P11-S5d.E' — R[A] = R[B][R[C]]. R[B] is Table.
                    // R[C] is a key — Int or Float are both fine
                    // (helper handles Float keys via `Table::get`,
                    // which normalises integral Floats back to the
                    // Int slot). A Table-typed key would be a
                    // semantics-level error PUC raises ("attempt to
                    // index with a table value" downstream); we bail
                    // the JIT path. R[A] is NOT forced to Int —
                    // 5.1/5.2 frontends often emit `LoadF R[C]=1.0`
                    // and then `GetTable R[A] = R[B][R[C]]` reusing
                    // R[A]=R[C]'s slot; forcing Int would conflict
                    // with the Float pin. The Variable stays Float
                    // and the emit bitcasts the i64 helper return
                    // back to F64 (`aligned_def`); a downstream
                    // Return1 / arith bitcasts F64→I64 to recover
                    // the raw payload bits.
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    let c = ins.c() as usize;
                    if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                        return None;
                    }
                    if matches!(reg_kinds[c], RegKind::Table)
                        || matches!(latest_writer_kind[c], RegKind::Table)
                        || maybe_table[c]
                    {
                        return None;
                    }
                    // P11-S5d.G — Nil key would call `Table::get(Nil)`
                    // which is well-defined (returns Nil) but the
                    // raw-payload contract breaks: 0 bits for Nil
                    // can't be distinguished from a valid `Int(0)`
                    // stored at that slot. Bail to interp.
                    if is_nil_writer[c] {
                        return None;
                    }
                    // Default-kind for GetTable destination depends on the
                    // dialect — luna's storage helper returns raw payload
                    // bits regardless of the slot's actual atag, and the
                    // method-JIT writeback uses reg_kinds[a] to pick the
                    // Value tag back. Under 5.1/5.2 (`float_only`) numbers
                    // are ALWAYS Float — `{10, 20, 30}` stores Float bits
                    // at each slot, so `t[i]` defaults to Float result.
                    // Under 5.3+ integer literals stay Int — default Int.
                    //
                    // NOTE: `pre53` (= version ≤ 5.3) is INCORRECT here
                    // — it includes 5.3 which has the integer subtype.
                    // Use `float_only` (= version ≤ 5.2) to gate the
                    // Float default. See
                    // `docs/known-bugs/fixed/jit-51-52-table-int-tag.md`
                    // and the 5.3 audit test
                    // `tests/jit_dialect_audit.rs::audit_gettable_computed_key`.
                    let default_kind = if float_only {
                        RegKind::Float
                    } else {
                        RegKind::Int
                    };
                    if matches!(reg_kinds[a], RegKind::Unset) {
                        reg_kinds[a] = default_kind;
                    }
                    latest_writer_kind[a] = default_kind;
                    maybe_table[a] = true;
                    is_nil_writer[a] = false;
                }
                Op::Len => {
                    // S5c — R[A] = #R[B]. R[B] Table; R[A] holds the
                    // Int length helper return.
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                        return None;
                    }
                    // P11-S5d.F — Len's i64 helper return goes through
                    // `aligned_def`'s bitcast on the writer side, so
                    // the slot's declared type need not be Int. A
                    // Float-pinned slot (5.1/5.2 reuse the ForPrep
                    // init slot for `#t` after the loop) is fine —
                    // the F64 Variable holds the i64 bits reinterpret,
                    // and the downstream `Return1` (whose emit bitcasts
                    // F64→I64 when the slot's declared Float) recovers
                    // them. Track the active write kind via
                    // `latest_writer_kind` so `ret_kind` derives from
                    // Len's Int, not from an earlier Float writer.
                    match reg_kinds[a] {
                        RegKind::Int | RegKind::Float => { /* keep declared */ }
                        RegKind::Unset => {
                            reg_kinds[a] = RegKind::Int;
                        }
                        RegKind::Table => return None,
                    }
                    latest_writer_kind[a] = RegKind::Int;
                    // `Len`'s result is always a real Int — clear
                    // any prior maybe_table tag.
                    maybe_table[a] = false;
                    is_nil_writer[a] = false;
                }
                _ => return None,
            }
            pc += 1;
        }
        if reg_kinds == pre_regs && ret_kind == pre_ret {
            break;
        }
    }
    // After convergence: derive per-arg kinds + the ret_is_float flag
    // for the cache slot. An arg that's still Unset (param read by
    // nothing) is treated as Int so the dispatcher's masking is
    // well-defined.
    //
    // P11-S5d — Table-typed params now go through the dispatcher's
    // `Value::Table` marshalling path (`arg_table_mask`); they
    // pass the raw `Gc<Table>` ptr as the i64 ABI slot. S5c's
    // earlier bail (no Table path in the dispatcher) is lifted.
    let mut arg_float_mask: u8 = 0;
    let mut arg_table_mask: u8 = 0;
    for i in 0..num_params {
        match reg_kinds[i] {
            RegKind::Float => arg_float_mask |= 1 << i,
            RegKind::Table => arg_table_mask |= 1 << i,
            _ => {}
        }
    }
    let ret_is_float = matches!(ret_kind, RegKind::Float);
    let ret_is_table = matches!(ret_kind, RegKind::Table);

    // P11-S5d.D step 2 — per-BB RegKind dataflow.
    //
    // `bb_entry_kinds[bb][r]` is the active kind (latest-writer kind on
    // every path reaching this BB) for register `r` at the BB's entry
    // PC. emit-time `current_kinds` resets to this on every BB switch
    // so an alternate-path writer's kind doesn't leak into the
    // current path. Step 4 will gate the readers behind this so
    // Float-vs-Int / Float-vs-Table register reuse across BBs can
    // unify globally (Float+Int unify is the gate for 5.1/5.2
    // binary_trees + table_alloc).
    //
    // Step 2 itself is functionally a near no-op: the only existing
    // reader of `current_kinds` is `Op::SetList`, which reads regs it
    // just wrote inside the same BB (writers always immediately
    // precede the SetList). The reset can't change SetList's view in
    // any currently-JIT'd shape; future readers added in step 4 will
    // depend on it.
    //
    // Lattice:
    //   TOP = `RegKind::Unset` (initial non-entry BB entry; encodes
    //         "no info yet" during fixpoint and "fall back to declared
    //         `reg_kinds`" at emit time).
    //   `Int` / `Float` / `Table` = definite kinds.
    //   meet(X, X) = X; meet(X, Unset) = X; meet(X, Y) for X ≠ Y =
    //   Unset (join conflict — emit's step-4 reader will fall back).
    //
    // Mirrors S5d.B's `defines_table` dataflow shape: forward, fixed
    // point with intersection-at-joins, non-entry BBs init at TOP, BB
    // 0 init from param kinds.
    let init_kind_for_reg = |i: usize| -> RegKind {
        if i < num_params {
            if (arg_float_mask >> i) & 1 == 1 {
                RegKind::Float
            } else if (arg_table_mask >> i) & 1 == 1 {
                RegKind::Table
            } else {
                RegKind::Int
            }
        } else {
            RegKind::Unset
        }
    };
    let meet_kind = |a: RegKind, b: RegKind| -> RegKind {
        match (a, b) {
            (RegKind::Unset, x) | (x, RegKind::Unset) => x,
            (x, y) if x == y => x,
            _ => RegKind::Unset,
        }
    };
    let body_apply_kinds = |bb_idx: usize, state: &mut Vec<RegKind>| {
        let bb_start = bb_pcs[bb_idx];
        let bb_end = bb_pcs.get(bb_idx + 1).copied().unwrap_or(n);
        for p in bb_start..bb_end {
            let ins = proto.code[p];
            // Math fold: the underlying GetField / Move / Call inside
            // a fold are skipped at emit (`pc += 3` after the
            // GetTabUp), so their would-be writes don't happen. Only
            // the GetTabUp at `start_pc` actually writes
            // `fold.dst_reg = Float`.
            if folded_math[p] {
                if let Some(fold) = math_folds.iter().find(|f| f.start_pc == p) {
                    if let Some(slot) = state.get_mut(fold.dst_reg as usize) {
                        *slot = RegKind::Float;
                    }
                }
                continue;
            }
            match ins.op() {
                Op::LoadI => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = RegKind::Int;
                    }
                }
                Op::LoadF => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = RegKind::Float;
                    }
                }
                Op::LoadK => {
                    let k = match proto.consts.get(ins.bx() as usize) {
                        Some(LuaValue::Float(_)) => RegKind::Float,
                        Some(LuaValue::Int(_)) => RegKind::Int,
                        _ => RegKind::Unset,
                    };
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = k;
                    }
                }
                Op::Move => {
                    let src_kind = state
                        .get(ins.b() as usize)
                        .copied()
                        .unwrap_or(RegKind::Unset);
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = src_kind;
                    }
                }
                Op::Add | Op::Sub | Op::Mul | Op::Div => {
                    // Result kind is picked from the sweep's
                    // `reg_kinds[a]` at emit (`current_kinds[a] = k`
                    // mirrors that). Replay the same here.
                    let k = reg_kinds
                        .get(ins.a() as usize)
                        .copied()
                        .unwrap_or(RegKind::Unset);
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = k;
                    }
                }
                Op::Call => {
                    // Self-recursive (the only non-folded Call shape
                    // the whitelist admits). Result is `ret_kind`.
                    if !matches!(ret_kind, RegKind::Unset) {
                        if let Some(slot) = state.get_mut(ins.a() as usize) {
                            *slot = ret_kind;
                        }
                    }
                }
                Op::ForPrep => {
                    let a = ins.a() as usize;
                    let is_float = matches!(
                        reg_kinds.get(a).copied().unwrap_or(RegKind::Unset),
                        RegKind::Float
                    );
                    match (pre53, is_float) {
                        (true, false) => {
                            if let Some(s) = state.get_mut(a) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 1) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 2) {
                                *s = RegKind::Int;
                            }
                        }
                        (false, false) => {
                            if let Some(s) = state.get_mut(a) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 1) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 2) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 3) {
                                *s = RegKind::Int;
                            }
                        }
                        (true, true) => {
                            if let Some(s) = state.get_mut(a) {
                                *s = RegKind::Float;
                            }
                            if let Some(s) = state.get_mut(a + 1) {
                                *s = RegKind::Float;
                            }
                            if let Some(s) = state.get_mut(a + 2) {
                                *s = RegKind::Int;
                            }
                        }
                        (false, true) => {
                            if let Some(s) = state.get_mut(a) {
                                *s = RegKind::Float;
                            }
                            if let Some(s) = state.get_mut(a + 1) {
                                *s = RegKind::Float;
                            }
                            if let Some(s) = state.get_mut(a + 2) {
                                *s = RegKind::Int;
                            }
                            if let Some(s) = state.get_mut(a + 3) {
                                *s = RegKind::Float;
                            }
                        }
                    }
                }
                Op::ForLoop => {
                    let a = ins.a() as usize;
                    let is_float = matches!(
                        reg_kinds.get(a).copied().unwrap_or(RegKind::Unset),
                        RegKind::Float
                    );
                    if is_float {
                        if let Some(s) = state.get_mut(a) {
                            *s = RegKind::Float;
                        }
                        if let Some(s) = state.get_mut(a + 3) {
                            *s = RegKind::Float;
                        }
                    } else if pre53 {
                        if let Some(s) = state.get_mut(a) {
                            *s = RegKind::Int;
                        }
                        if let Some(s) = state.get_mut(a + 3) {
                            *s = RegKind::Int;
                        }
                    } else {
                        if let Some(s) = state.get_mut(a) {
                            *s = RegKind::Int;
                        }
                        if let Some(s) = state.get_mut(a + 1) {
                            *s = RegKind::Int;
                        }
                        if let Some(s) = state.get_mut(a + 3) {
                            *s = RegKind::Int;
                        }
                    }
                }
                Op::NewTable => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = RegKind::Table;
                    }
                }
                Op::GetI | Op::GetTable => {
                    // Emit writes `current_kinds[a] = reg_kinds[a]`
                    // (the declared kind picked by the sweep, since
                    // GetI/GetTable's helper returns raw payload
                    // bits that could be Int, Float or Table at
                    // runtime — the sweep + `maybe_table` tracker
                    // handles the ambiguity downstream).
                    let k = reg_kinds
                        .get(ins.a() as usize)
                        .copied()
                        .unwrap_or(RegKind::Unset);
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = k;
                    }
                }
                Op::LoadNil => {
                    // P11-S5d.G — emit writes iconst(0) into each
                    // `R[A..=A+B]` slot. The declared kind (Int by
                    // the sweep's Unset→Int default, or whatever a
                    // prior writer pinned) stays. The emit-side
                    // `current_is_nil` shadow (reset at every BB
                    // switch, set true here, cleared by other emit
                    // writers) is the SetList disambiguation signal.
                    let k = reg_kinds
                        .get(ins.a() as usize)
                        .copied()
                        .unwrap_or(RegKind::Int);
                    let a = ins.a() as usize;
                    for off in 0..=(ins.b() as usize) {
                        if let Some(slot) = state.get_mut(a + off) {
                            *slot = k;
                        }
                    }
                }
                Op::Len => {
                    if let Some(slot) = state.get_mut(ins.a() as usize) {
                        *slot = RegKind::Int;
                    }
                }
                // GetUpval emits a placeholder def_var(0) but does
                // not update `current_kinds` (the matching Call
                // reads `reg_kinds`, not `current_kinds`). Mirror
                // that here — no state change.
                // SetTable / SetList write through R[A]; R[A] stays
                // whatever it was.
                _ => {}
            }
        }
    };

    let mut bb_entry_kinds: Vec<Vec<RegKind>> = (0..num_bbs)
        .map(|_| vec![RegKind::Unset; max_stack])
        .collect();
    let mut bb_exit_kinds: Vec<Vec<RegKind>> = (0..num_bbs)
        .map(|_| vec![RegKind::Unset; max_stack])
        .collect();
    for i in 0..max_stack {
        bb_entry_kinds[0][i] = init_kind_for_reg(i);
    }
    let max_iters_kinds = num_bbs * (max_stack + 2);
    let mut iters_kinds = 0;
    let mut changed_kinds = true;
    while changed_kinds && iters_kinds < max_iters_kinds {
        changed_kinds = false;
        iters_kinds += 1;
        for bb_idx in 0..num_bbs {
            let new_entry = if bb_predecessors[bb_idx].is_empty() {
                bb_entry_kinds[bb_idx].clone()
            } else {
                let mut e = bb_exit_kinds[bb_predecessors[bb_idx][0]].clone();
                for &pred in &bb_predecessors[bb_idx][1..] {
                    for (i, val) in bb_exit_kinds[pred].iter().enumerate() {
                        e[i] = meet_kind(e[i], *val);
                    }
                }
                if bb_idx == 0 {
                    for i in 0..max_stack {
                        e[i] = init_kind_for_reg(i);
                    }
                }
                e
            };
            let mut state = new_entry.clone();
            body_apply_kinds(bb_idx, &mut state);
            if state != bb_exit_kinds[bb_idx] {
                bb_exit_kinds[bb_idx] = state;
                changed_kinds = true;
            }
            if new_entry != bb_entry_kinds[bb_idx] {
                bb_entry_kinds[bb_idx] = new_entry;
                changed_kinds = true;
            }
        }
    }

    let mut sig = module.make_signature();
    for _ in 0..num_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let fn_id = module
        .declare_function("luna_jit_chunk", Linkage::Local, &sig)
        .ok()?;

    let mut ctx = module.make_context();
    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, fn_id.as_u32());

    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

    // Create one cranelift Block per Lua basic block, indexed by the
    // BB's leading PC. The entry block also gets the Variable-declaration
    // prelude so the chunk has well-defined register values from PC 0.
    let mut pc_to_block: Vec<Option<Block>> = vec![None; n];
    for pc_i in 0..n {
        if bb_starts[pc_i] {
            pc_to_block[pc_i] = Some(bcx.create_block());
        }
    }
    let entry = pc_to_block[0].expect("entry block exists");
    // Append the entry block's function-param block params before
    // switching in, so the params arrive as block args we can read
    // straight into the register Variables.
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);

    // Variables = Lua registers, declared once on the entry block so
    // every BB downstream can `use_var` / `def_var` them. Each register
    // gets a Cranelift type chosen from `reg_kinds[i]` (Int → I64,
    // Float → F64). Unset registers default to I64 — they're unreachable
    // in well-formed Lua but we still need a valid SSA shape.
    let max_stack = (proto.max_stack as usize).max(num_params);
    let mut regs: Vec<Variable> = Vec::with_capacity(max_stack);
    let entry_block_params: Vec<_> = bcx.block_params(entry).to_vec();
    for i in 0..max_stack {
        let cl_ty = match reg_kinds.get(i).copied().unwrap_or(RegKind::Unset) {
            RegKind::Float => types::F64,
            // Table is a `Gc<Table>` pointer pun — I64-shaped at the
            // Cranelift level, distinct in the kind lattice.
            RegKind::Int | RegKind::Unset | RegKind::Table => types::I64,
        };
        let v = bcx.declare_var(cl_ty);
        // Lua call ABI: arg `i` lands in register `i`. The cranelift
        // entry signature is i64; for a Float param we bitcast the i64
        // bit-pattern back to f64 here. Params past num_params are
        // zero-initialised in their target type.
        let init = if i < num_params {
            let raw = entry_block_params[i];
            if cl_ty == types::F64 {
                bcx.ins().bitcast(types::F64, MemFlags::new(), raw)
            } else {
                raw
            }
        } else if cl_ty == types::F64 {
            bcx.ins().f64const(0.0)
        } else {
            bcx.ins().iconst(types::I64, 0)
        };
        bcx.def_var(v, init);
        regs.push(v);
    }

    // P11-S5d.C — emit-side per-PC kind tracker. Initialized from
    // the per-arg masks (Float bit → Float, Table bit → Table, else
    // Int) and updated forward at every writer op below. Used by
    // `SetList` to tag-store each element correctly and by
    // arith/cmp ops in lieu of the global `reg_kinds` slot when the
    // global slot has been "joint-pinned" by Int + Table re-use.
    let mut current_kinds: Vec<RegKind> = vec![RegKind::Unset; max_stack];
    for i in 0..num_params {
        current_kinds[i] = if (arg_float_mask >> i) & 1 == 1 {
            RegKind::Float
        } else if (arg_table_mask >> i) & 1 == 1 {
            RegKind::Table
        } else {
            RegKind::Int
        };
    }
    // P11-S5d.G — parallel to `current_kinds`: tracks "the value
    // last written here is a Nil sentinel (raw bits = 0)". Set by
    // `Op::LoadNil` emit; cleared by any other writer touching the
    // same register. Reset to all-false at every BB switch (the
    // narrow LoadNil → SetList window we lower lives entirely in
    // one BB; broader BB-level Nil dataflow is left for later if
    // a wider pattern needs it). `SetList` emit reads this to pick
    // `RAW_TAG_NIL` over the default Int tag, so a chunk like
    // `binary_trees`'s `{nil, nil}` leaf stores actual Nil values
    // instead of misinterpreting the 0 bits as `Int(0)`.
    let mut current_is_nil: Vec<bool> = vec![false; max_stack];

    let mut current_block = entry;
    let mut terminated = false;
    let mut pc = 0;
    while pc < n {
        // Entering a new BB: if the previous BB fell through without a
        // terminator, append an explicit jump so cranelift's verifier
        // doesn't choke.
        if pc != 0 && bb_starts[pc] {
            let next_blk = pc_to_block[pc].expect("BB present");
            if !terminated {
                bcx.ins().jump(next_blk, &[]);
            }
            bcx.switch_to_block(next_blk);
            current_block = next_blk;
            terminated = false;
            // P11-S5d.D step 2 — reset emit-side `current_kinds` to
            // the per-BB dataflow result so an alternate-path
            // writer's kind doesn't leak forward. The linear writer
            // updates below continue to refine `current_kinds` as
            // emit progresses through the new BB.
            let new_bb_idx = pc_to_bb[pc];
            current_kinds = bb_entry_kinds[new_bb_idx].clone();
            // P11-S5d.G — Nil writes don't cross BB joins in the
            // patterns we lower; reset rather than fold them into
            // a separate per-BB dataflow.
            for slot in current_is_nil.iter_mut() {
                *slot = false;
            }
        }
        let _ = current_block; // tracked only for parity assertions in tests.
        let ins = proto.code[pc];
        let a_kind = |k: &[RegKind], idx: u32| k.get(idx as usize).copied().unwrap_or(RegKind::Int);
        match ins.op() {
            Op::LoadI => {
                let imm = ins.sbx() as i64;
                let v = bcx.ins().iconst(types::I64, imm);
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, v);
                current_kinds[ins.a() as usize] = RegKind::Int;
                current_is_nil[ins.a() as usize] = false;
            }
            Op::LoadF => {
                let f = ins.sbx() as f64;
                let v = bcx.ins().f64const(f);
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, v);
                current_kinds[ins.a() as usize] = RegKind::Float;
                current_is_nil[ins.a() as usize] = false;
            }
            Op::LoadK => {
                // Whitelist ensures Int or Float const.
                let bx = ins.bx() as usize;
                let (v, k) = match proto.consts[bx] {
                    LuaValue::Float(f) => (bcx.ins().f64const(f), RegKind::Float),
                    LuaValue::Int(i) => (bcx.ins().iconst(types::I64, i), RegKind::Int),
                    _ => unreachable!("scanner rejects non-numeric LoadK"),
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, v);
                current_kinds[ins.a() as usize] = k;
                current_is_nil[ins.a() as usize] = false;
            }
            Op::LoadNil => {
                // P11-S5d.G — `R[A..=A+B] = nil`. Lower to a sequence of
                // `iconst(0)` writes, then flag `current_is_nil` so the
                // matching SetList in this BB picks `RAW_TAG_NIL` over
                // the default Int tag. The `aligned_def` accepts any
                // declared kind because the 8-byte payload of Nil is 0
                // (lossless bitcast to F64 or I64).
                let zero = bcx.ins().iconst(types::I64, 0);
                let a = ins.a() as usize;
                for off in 0..=(ins.b() as usize) {
                    let r = a + off;
                    aligned_def(&mut bcx, &regs, &reg_kinds, r, zero);
                    current_is_nil[r] = true;
                }
            }
            Op::Move => {
                let src = bcx.use_var(regs[ins.b() as usize]);
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, src);
                current_kinds[ins.a() as usize] = current_kinds[ins.b() as usize];
                current_is_nil[ins.a() as usize] = current_is_nil[ins.b() as usize];
            }
            Op::Add | Op::Sub | Op::Mul | Op::Div => {
                let lhs = bcx.use_var(regs[ins.b() as usize]);
                let rhs = bcx.use_var(regs[ins.c() as usize]);
                // Destination kind picked from the sweep's final
                // `reg_kinds` — `current_kinds[a]` reflects pre-write
                // state and may still be Unset before this op runs.
                let k = a_kind(&reg_kinds, ins.a());
                let r = match (ins.op(), k) {
                    (Op::Add, RegKind::Float) => bcx.ins().fadd(lhs, rhs),
                    (Op::Sub, RegKind::Float) => bcx.ins().fsub(lhs, rhs),
                    (Op::Mul, RegKind::Float) => bcx.ins().fmul(lhs, rhs),
                    (Op::Div, RegKind::Float) => bcx.ins().fdiv(lhs, rhs),
                    (Op::Add, _) => bcx.ins().iadd(lhs, rhs),
                    (Op::Sub, _) => bcx.ins().isub(lhs, rhs),
                    (Op::Mul, _) => bcx.ins().imul(lhs, rhs),
                    (Op::Div, _) => unreachable!("Op::Div scan pins result to Float"),
                    _ => unreachable!(),
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, r);
                current_kinds[ins.a() as usize] = k;
                current_is_nil[ins.a() as usize] = false;
            }
            Op::Return1 => {
                let v = bcx.use_var(regs[ins.a() as usize]);
                let out = if matches!(a_kind(&reg_kinds, ins.a()), RegKind::Float) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), v)
                } else {
                    v
                };
                bcx.ins().return_(&[out]);
                terminated = true;
            }
            Op::Return0 => {
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.ins().return_(&[zero]);
                terminated = true;
            }
            Op::Jmp => {
                let tgt = jmp_target(pc, ins);
                let tgt_blk = pc_to_block[tgt].expect("Jmp target is BB start");
                bcx.ins().jump(tgt_blk, &[]);
                terminated = true;
            }
            Op::GetTabUp => {
                // P11-S5b — emit-side fold consumer. PCs +1..+3 are
                // also folded; the outer loop advances `pc` by 3 (plus
                // the trailing `pc += 1`) so we skip past `GetField`,
                // `Move`, and the `Call`.
                debug_assert!(
                    folded_math[pc],
                    "scanner accepts GetTabUp only inside a math fold"
                );
                let fold = math_folds
                    .iter()
                    .find(|f| f.start_pc == pc)
                    .copied()
                    .expect("math fold for this PC");

                let mut libm_sig = module.make_signature();
                libm_sig.params.push(AbiParam::new(types::F64));
                libm_sig.returns.push(AbiParam::new(types::F64));
                let libm_id = module
                    .declare_function(fold.fn_name, Linkage::Import, &libm_sig)
                    .ok()?;
                let libm_ref = module.declare_func_in_func(libm_id, bcx.func);

                let arg_kind = a_kind(&reg_kinds, fold.arg_reg);
                let arg_var = bcx.use_var(regs[fold.arg_reg as usize]);
                let arg_f64 = match arg_kind {
                    RegKind::Float => arg_var,
                    RegKind::Int | RegKind::Unset => bcx.ins().fcvt_from_sint(types::F64, arg_var),
                    // The fold's `Move` source can only be a Lua
                    // numeric — the whitelist's `Op::Call B=2` gate
                    // implies a numeric arg. A Table-typed source
                    // would have been bailed earlier by the kind
                    // sweep mismatching the fold's Float result.
                    RegKind::Table => unreachable!("math fold arg can't be Table"),
                };
                let call_inst = bcx.ins().call(libm_ref, &[arg_f64]);
                let result_f64 = bcx.inst_results(call_inst)[0];
                aligned_def(
                    &mut bcx,
                    &regs,
                    &reg_kinds,
                    fold.dst_reg as usize,
                    result_f64,
                );
                current_kinds[fold.dst_reg as usize] = RegKind::Float;
                current_is_nil[fold.dst_reg as usize] = false;

                pc += 3; // skip GetField + Move + Call; outer `pc += 1` lands past the Call.
            }
            Op::GetField if folded_math[pc] => {
                unreachable!("GetTabUp emit advances pc past the rest of the fold");
            }
            Op::GetUpval => {
                let a = ins.a() as usize;
                if is_upval_value_read[pc] {
                    // P11-S5d.J — ValueRead: fetch the upvalue at
                    // runtime via `luna_jit_upval_get`. The dispatcher
                    // has pinned `JIT_CL` to the active closure for
                    // this entry, so the helper can resolve the
                    // upvalue cell. Result is the raw 8-byte payload;
                    // `aligned_def` bitcasts to F64 since the sweep
                    // pinned reg_kinds[a] = Float.
                    let idx_arg = bcx.ins().iconst(types::I64, ins.b() as i64);
                    let mut sig = module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function("luna_jit_upval_get", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let call_inst = bcx.ins().call(r, &[idx_arg]);
                    let v = bcx.inst_results(call_inst)[0];
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                    current_kinds[a] = reg_kinds[a];
                    current_is_nil[a] = false;
                } else {
                    // S2c.C — SelfMarker placeholder. The matching
                    // Op::Call gets rewritten to a direct cranelift
                    // call; this register's value is never read.
                    let zero = if matches!(a_kind(&reg_kinds, ins.a()), RegKind::Float) {
                        bcx.ins().f64const(0.0)
                    } else {
                        bcx.ins().iconst(types::I64, 0)
                    };
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, zero);
                }
            }
            Op::Call => {
                debug_assert!(
                    self_call_pcs[pc],
                    "scanner accepts only self-recursive Calls"
                );
                let a = ins.a() as usize;
                let nargs = (ins.b() - 1) as usize;
                let mut arg_vals: Vec<Value> = Vec::with_capacity(nargs);
                for i in 0..nargs {
                    let slot_idx = a + 1 + i;
                    let v = bcx.use_var(regs[slot_idx]);
                    // The cranelift call sig matches the entry sig
                    // (all i64). Bitcast Float args back to i64 at
                    // the call boundary.
                    let v_i64 = if matches!(a_kind(&reg_kinds, slot_idx as u32), RegKind::Float) {
                        bcx.ins().bitcast(types::I64, MemFlags::new(), v)
                    } else {
                        v
                    };
                    arg_vals.push(v_i64);
                }
                let self_ref = module.declare_func_in_func(fn_id, bcx.func);
                let call_inst = bcx.ins().call(self_ref, &arg_vals);
                let result_i64 = bcx.inst_results(call_inst)[0];
                // Self-call result is `ret_kind`; bitcast back to
                // F64 if Float. Pre-write `current_kinds[a]` would
                // be stale here.
                let result = if matches!(ret_kind, RegKind::Float) {
                    bcx.ins().bitcast(types::F64, MemFlags::new(), result_i64)
                } else {
                    result_i64
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, a, result);
                // P11-S5d.C — self-recursive call returns ret_kind.
                if !matches!(ret_kind, RegKind::Unset) {
                    current_kinds[a] = ret_kind;
                }
                current_is_nil[a] = false;
            }
            Op::ForPrep => {
                let &(_, loop_pc, step_imm) = for_loops
                    .iter()
                    .find(|&&(p, _, _)| p == pc)
                    .expect("scanner recorded this ForPrep");
                let a = ins.a() as usize;
                let is_float = matches!(a_kind(&reg_kinds, ins.a()), RegKind::Float);
                let step_i = bcx.ins().iconst(types::I64, step_imm);

                match (pre53, is_float) {
                    (true, false) => {
                        // S5a.B — pre-5.3 Int form. R[A] = init - step
                        // (so ForLoop's first add lands on init), copy
                        // limit + step over, unconditional jump to the
                        // ForLoop block. R[A+3] left alone — pre53
                        // ForLoop writes it on continue.
                        let init = bcx.use_var(regs[a]);
                        let limit = bcx.use_var(regs[a + 1]);
                        let pre = bcx.ins().isub(init, step_i);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a, pre);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 1, limit);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 2, step_i);
                        current_kinds[a] = RegKind::Int;
                        current_kinds[a + 1] = RegKind::Int;
                        current_kinds[a + 2] = RegKind::Int;
                        let loop_blk = pc_to_block[loop_pc].expect("ForLoop BB");
                        bcx.ins().jump(loop_blk, &[]);
                        terminated = true;
                    }
                    (false, false) => {
                        // S5a — 5.4+ Int count form.
                        let init = bcx.use_var(regs[a]);
                        let limit = bcx.use_var(regs[a + 1]);

                        let empty = if step_imm > 0 {
                            bcx.ins().icmp(IntCC::SignedGreaterThan, init, limit)
                        } else {
                            bcx.ins().icmp(IntCC::SignedLessThan, init, limit)
                        };

                        // count = (limit - init) / step (positive-step)
                        //       = (init - limit) / -step (negative-step)
                        // Both branches yield a non-negative count.
                        let span = if step_imm > 0 {
                            bcx.ins().isub(limit, init)
                        } else {
                            bcx.ins().isub(init, limit)
                        };
                        let abs_step = bcx.ins().iconst(types::I64, step_imm.abs());
                        let count = bcx.ins().sdiv(span, abs_step);

                        aligned_def(&mut bcx, &regs, &reg_kinds, a, init);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 1, count);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 2, step_i);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 3, init);
                        current_kinds[a] = RegKind::Int;
                        current_kinds[a + 1] = RegKind::Int;
                        current_kinds[a + 2] = RegKind::Int;
                        current_kinds[a + 3] = RegKind::Int;

                        let body_blk = pc_to_block[pc + 1].expect("body BB start");
                        let exit_blk = pc_to_block[loop_pc + 1].expect("exit BB start");
                        bcx.ins().brif(empty, exit_blk, &[], body_blk, &[]);
                        terminated = true;
                    }
                    (true, true) => {
                        // S5a.C — pre-5.3 Float form. R[A] = init - step,
                        // R[A+1] = limit, R[A+2] = step, unconditional
                        // jump to the ForLoop block. step_imm is the
                        // (Int) immediate the bytecode put in R[A+2];
                        // we promote it to f64 for arith and write its
                        // Int bit-pattern to R[A+2]'s declared Int slot.
                        let init = bcx.use_var(regs[a]);
                        let limit = bcx.use_var(regs[a + 1]);
                        let step_f = bcx.ins().f64const(step_imm as f64);
                        let pre = bcx.ins().fsub(init, step_f);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a, pre);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 1, limit);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 2, step_i);
                        current_kinds[a] = RegKind::Float;
                        current_kinds[a + 1] = RegKind::Float;
                        current_kinds[a + 2] = RegKind::Int;
                        let loop_blk = pc_to_block[loop_pc].expect("ForLoop BB");
                        bcx.ins().jump(loop_blk, &[]);
                        terminated = true;
                    }
                    (false, true) => {
                        // S5a.C — 5.4+ Float form. Mirrors interp's
                        // post53 Float branch in `for_prep`: empty test
                        // `init > limit` (positive step) / `init < limit`
                        // (negative step), and on continue write R[A] =
                        // init, R[A+1] = limit, R[A+2] = step, R[A+3] =
                        // init, fall through to body. No count form for
                        // Float — R[A+1] keeps the limit, not a
                        // remaining-count.
                        let init = bcx.use_var(regs[a]);
                        let limit = bcx.use_var(regs[a + 1]);
                        let step_f = bcx.ins().f64const(step_imm as f64);

                        let empty = if step_imm > 0 {
                            bcx.ins().fcmp(FloatCC::GreaterThan, init, limit)
                        } else {
                            bcx.ins().fcmp(FloatCC::LessThan, init, limit)
                        };

                        let set_blk = bcx.create_block();
                        let exit_blk = pc_to_block[loop_pc + 1].expect("exit BB start");
                        bcx.ins().brif(empty, exit_blk, &[], set_blk, &[]);
                        terminated = true;

                        bcx.switch_to_block(set_blk);
                        bcx.seal_block(set_blk);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a, init);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 1, limit);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 2, step_i);
                        aligned_def(&mut bcx, &regs, &reg_kinds, a + 3, init);
                        current_kinds[a] = RegKind::Float;
                        current_kinds[a + 1] = RegKind::Float;
                        current_kinds[a + 2] = RegKind::Int;
                        current_kinds[a + 3] = RegKind::Float;
                        let _ = step_f;
                        let body_blk = pc_to_block[pc + 1].expect("body BB start");
                        bcx.ins().jump(body_blk, &[]);
                    }
                }
            }
            Op::ForLoop => {
                let prep_pc = for_loops
                    .iter()
                    .find(|&&(_, lp, _)| lp == pc)
                    .map(|&(p, _, _)| p)
                    .expect("scanner paired this ForLoop");
                let &(_, _, step_imm) = for_loops
                    .iter()
                    .find(|&&(p, _, _)| p == prep_pc)
                    .expect("step_const recorded");
                let a = ins.a() as usize;
                let is_float = matches!(a_kind(&reg_kinds, ins.a()), RegKind::Float);

                if is_float {
                    // S5a.C — Float ForLoop. Same shape for pre53 and
                    // post53 (Float Loop never used the count form).
                    // next = R[A] + step; cont = next ≤ limit (positive)
                    // / next ≥ limit (negative). On continue → R[A] =
                    // next, R[A+3] = next, back-jump to body.
                    let cur = bcx.use_var(regs[a]);
                    let step_f = bcx.ins().f64const(step_imm as f64);
                    let next = bcx.ins().fadd(cur, step_f);
                    let limit = bcx.use_var(regs[a + 1]);
                    let cont = if step_imm > 0 {
                        bcx.ins().fcmp(FloatCC::LessThanOrEqual, next, limit)
                    } else {
                        bcx.ins().fcmp(FloatCC::GreaterThanOrEqual, next, limit)
                    };
                    let continue_blk = bcx.create_block();
                    let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                    let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                    bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                    terminated = true;

                    bcx.switch_to_block(continue_blk);
                    bcx.seal_block(continue_blk);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, next);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a + 3, next);
                    current_kinds[a] = RegKind::Float;
                    current_kinds[a + 3] = RegKind::Float;
                    bcx.ins().jump(body_blk, &[]);
                } else if pre53 {
                    // S5a.B — pre-5.3 Int form. R[A] += step; check vs
                    // R[A+1] = limit; continue → write R[A+3] = R[A]
                    // + backward jump.
                    let cur = bcx.use_var(regs[a]);
                    let step_v = bcx.ins().iconst(types::I64, step_imm);
                    let next = bcx.ins().iadd(cur, step_v);
                    let limit = bcx.use_var(regs[a + 1]);
                    let cont = if step_imm > 0 {
                        bcx.ins().icmp(IntCC::SignedLessThanOrEqual, next, limit)
                    } else {
                        bcx.ins().icmp(IntCC::SignedGreaterThanOrEqual, next, limit)
                    };
                    let continue_blk = bcx.create_block();
                    let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                    let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                    bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                    terminated = true;

                    bcx.switch_to_block(continue_blk);
                    bcx.seal_block(continue_blk);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, next);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a + 3, next);
                    current_kinds[a] = RegKind::Int;
                    current_kinds[a + 3] = RegKind::Int;
                    bcx.ins().jump(body_blk, &[]);
                } else {
                    // S5a — 5.4+ Int count form.
                    let count = bcx.use_var(regs[a + 1]);
                    let zero_i = bcx.ins().iconst(types::I64, 0);
                    let cont = bcx.ins().icmp(IntCC::SignedGreaterThan, count, zero_i);

                    let continue_blk = bcx.create_block();
                    let body_blk = pc_to_block[prep_pc + 1].expect("body BB");
                    let exit_blk = pc_to_block[pc + 1].expect("exit BB");
                    bcx.ins().brif(cont, continue_blk, &[], exit_blk, &[]);
                    terminated = true;

                    bcx.switch_to_block(continue_blk);
                    bcx.seal_block(continue_blk);
                    let cur = bcx.use_var(regs[a]);
                    let step_v = bcx.ins().iconst(types::I64, step_imm);
                    let next = bcx.ins().iadd(cur, step_v);
                    let one = bcx.ins().iconst(types::I64, 1);
                    let new_count = bcx.ins().isub(count, one);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, next);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a + 1, new_count);
                    aligned_def(&mut bcx, &regs, &reg_kinds, a + 3, next);
                    current_kinds[a] = RegKind::Int;
                    current_kinds[a + 1] = RegKind::Int;
                    current_kinds[a + 3] = RegKind::Int;
                    bcx.ins().jump(body_blk, &[]);
                }
            }
            Op::Lt | Op::Le | Op::Eq => {
                let jmp = proto.code[pc + 1];
                debug_assert!(matches!(jmp.op(), Op::Jmp), "scanner enforces pairing");
                let lhs = bcx.use_var(regs[ins.a() as usize]);
                let rhs = bcx.use_var(regs[ins.b() as usize]);
                let lhs_kind = a_kind(&reg_kinds, ins.a());
                let rhs_kind = a_kind(&reg_kinds, ins.b());
                // Operand kinds were unified by the scan; if either is
                // Float they both are.
                let cond =
                    if matches!(lhs_kind, RegKind::Float) || matches!(rhs_kind, RegKind::Float) {
                        let fcc = match ins.op() {
                            Op::Lt => FloatCC::LessThan,
                            Op::Le => FloatCC::LessThanOrEqual,
                            Op::Eq => FloatCC::Equal,
                            _ => unreachable!(),
                        };
                        bcx.ins().fcmp(fcc, lhs, rhs)
                    } else {
                        let icc = match ins.op() {
                            Op::Lt => IntCC::SignedLessThan,
                            Op::Le => IntCC::SignedLessThanOrEqual,
                            Op::Eq => IntCC::Equal,
                            _ => unreachable!(),
                        };
                        bcx.ins().icmp(icc, lhs, rhs)
                    };
                // PUC `cond_skip`: bump_pc (skip the Jmp) if cond != k;
                // otherwise execute the Jmp. So `cond == k` → take Jmp;
                // `cond != k` → fall through past Jmp.
                let fall_blk = pc_to_block[pc + 2].expect("fallthrough BB");
                let jmp_blk = pc_to_block[jmp_target(pc + 1, jmp)].expect("Jmp target BB");
                if ins.k() {
                    // k=true: take jmp when cond=1; fall when cond=0.
                    bcx.ins().brif(cond, jmp_blk, &[], fall_blk, &[]);
                } else {
                    // k=false: take jmp when cond=0; fall when cond=1.
                    bcx.ins().brif(cond, fall_blk, &[], jmp_blk, &[]);
                }
                terminated = true;
                pc += 1; // consume the paired Jmp; outer increment moves past it
            }
            Op::NewTable => {
                // P11-S5c — `R[A] = {}` lowers to a call into the
                // `luna_jit_new_table` Rust helper. The helper reads
                // the active Vm pointer from the thread-local set by
                // `enter_jit`. Result is the `Gc<Table>` pointer
                // pun'd to I64, written into R[A].
                //
                // P11-S5c.B — when the scan recorded a presize hint
                // (the NewTable opens a counted `for i = 1, N`
                // window), reach for the `_sized` variant with N
                // as an i64 const arg. Skips the O(log N) rehash
                // chain that would otherwise dominate the loop.
                //
                // P11-S5d.B — also honour `NewTable.B` as a presize
                // hint: luna's frontend emits `NewTable A B=N` for
                // `{a, b, c, ...}` literals (the SetList that
                // follows fills exactly N entries). Either source —
                // S5c.B window or NewTable.B — feeds the sized
                // helper; the explicit window wins on overlap.
                let presize = presize_for_newtable.get(&pc).copied().or_else(|| {
                    let b = ins.b();
                    if b > 0 { Some(b as i64) } else { None }
                });
                let g = if let Some(n) = presize {
                    let mut sig = module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.returns.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function("luna_jit_new_table_sized", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let n_v = bcx.ins().iconst(types::I64, n);
                    let call_inst = bcx.ins().call(r, &[n_v]);
                    bcx.inst_results(call_inst)[0]
                } else {
                    let mut sig = module.make_signature();
                    sig.returns.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function("luna_jit_new_table", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let call_inst = bcx.ins().call(r, &[]);
                    bcx.inst_results(call_inst)[0]
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, ins.a() as usize, g);
                current_kinds[ins.a() as usize] = RegKind::Table;
                current_is_nil[ins.a() as usize] = false;
            }
            Op::SetTable => {
                // P11-S5c — `R[A][R[B]] = R[C]`. Pick the Int/Int vs
                // Float/Float helper at emit time based on R[B]'s
                // resolved kind (the scan pinned R[B] and R[C] to
                // the same kind).
                //
                // P11-S5c.C — for the Int/Int variant, emit an inline
                // aset fast path: skip the helper call when the key
                // falls inside the table's array part. The cranelift
                // IR reads `atags.len`, `atags.ptr`, `avals.ptr`
                // straight from the `Gc<Table>` raw ptr (`#[repr(C)]`
                // + `offset_of!` make the layout stable), branches on
                // `(key - 1) as u64 < atags.len`, and either writes
                // the tag byte + i64 payload in-place or falls
                // through to the slow-path helper.
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let c = ins.c() as usize;
                let t_raw = bcx.use_var(regs[a]);
                // P11-S5d.D step 3+4 — when `R[A]` is Float-declared
                // because of a same-slot Float writer in another BB
                // (the binary_trees 5.1/5.2 pattern), `use_var` hands
                // back F64. Bitcast back to I64 so the inline aset
                // load / helper call sees a real `Gc<Table>` ptr.
                // Lossless reinterpret — `aligned_def` did the
                // matching F64→I64 bitcast at the NewTable write.
                let t = if matches!(
                    reg_kinds.get(a).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), t_raw)
                } else {
                    t_raw
                };
                let key = bcx.use_var(regs[b]);
                let val = bcx.use_var(regs[c]);
                let is_float = matches!(a_kind(&reg_kinds, b as u32), RegKind::Float);

                if !is_float {
                    // Inline aset fast path (Int key + Int val).
                    // P11-S5d.H — load `asize` (u64) once for both the
                    // in-range check and the `atags_ptr = avals_ptr +
                    // asize * 8` computation. Avals occupy `slab` from
                    // offset 0; atags trail at byte offset `asize * 8`.
                    let asize = bcx.ins().load(
                        types::I64,
                        MemFlags::trusted(),
                        t,
                        TABLE_ASIZE_OFFSET as i32,
                    );
                    let one = bcx.ins().iconst(types::I64, 1);
                    let key_minus_1 = bcx.ins().isub(key, one);
                    // `(key - 1) as u64 < asize` handles both
                    // `key >= 1` (else underflow → > any len) and
                    // `key <= asize` in one unsigned compare.
                    let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);

                    let fast_blk = bcx.create_block();
                    let slow_blk = bcx.create_block();
                    let merge_blk = bcx.create_block();
                    bcx.ins().brif(in_range, fast_blk, &[], slow_blk, &[]);

                    bcx.switch_to_block(fast_blk);
                    bcx.seal_block(fast_blk);
                    let avals_ptr = bcx.ins().load(
                        types::I64,
                        MemFlags::trusted(),
                        t,
                        TABLE_ARRAY_PTR_OFFSET as i32,
                    );
                    // atags_ptr = avals_ptr + asize * 8
                    let three = bcx.ins().iconst(types::I64, 3);
                    let avals_bytes = bcx.ins().ishl(asize, three);
                    let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
                    let tag_dst = bcx.ins().iadd(atags_ptr, key_minus_1);
                    let tag_byte = bcx.ins().iconst(types::I8, RAW_TAG_INT);
                    bcx.ins().store(MemFlags::trusted(), tag_byte, tag_dst, 0);
                    let val_off = bcx.ins().ishl(key_minus_1, three); // *8
                    let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                    bcx.ins().store(MemFlags::trusted(), val, val_dst, 0);
                    bcx.ins().jump(merge_blk, &[]);

                    bcx.switch_to_block(slow_blk);
                    bcx.seal_block(slow_blk);
                    let mut sig = module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function("luna_jit_table_set_int", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let _ = bcx.ins().call(r, &[t, key, val]);
                    bcx.ins().jump(merge_blk, &[]);

                    bcx.switch_to_block(merge_blk);
                    bcx.seal_block(merge_blk);
                } else {
                    // Float/Float — keep the helper-call form. The
                    // inline aset path stores raw Int tag + bits,
                    // which would mis-normalise integral floats (PUC
                    // semantics demand `t[1.0] = 1.0` lands in the
                    // Int(1) array slot, not in a Float-tagged hash
                    // entry); `Table::set` does the normalisation.
                    let key_i = bcx.ins().bitcast(types::I64, MemFlags::new(), key);
                    let val_i = bcx.ins().bitcast(types::I64, MemFlags::new(), val);
                    let mut sig = module.make_signature();
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    sig.params.push(AbiParam::new(types::I64));
                    let id = module
                        .declare_function("luna_jit_table_set_float_float", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let _ = bcx.ins().call(r, &[t, key_i, val_i]);
                }
            }
            Op::SetList => {
                // P11-S5d.B — `R[A][1..=B] = R[A+1..A+B]`. Inline
                // each store via the same atags/avals fast-path the
                // SetTable inline aset uses, since the table was
                // just freshly NewTable'd (with B as the array
                // presize) and the key range is guaranteed in
                // bounds. Each element's tag is picked at emit time
                // from `RegKind[A+i]`:
                //   Int     → raw::INT     (i64 verbatim)
                //   Float   → raw::FLOAT   (bitcast f64 → i64)
                //   Table   → raw::TABLE   (i64 ptr verbatim)
                //
                // P11-S5d.C — `B == 0` variadic form: the matching
                // preceding `Op::Call C=0` returns exactly 1 value
                // (the self-recursive callee's `returns_one == true`
                // guarantee), so the static count is
                // `A_call - A_list`. Source regs are still
                // `R[A+1..A+count]`.
                let a = ins.a() as usize;
                let b_field = ins.b();
                let b = if b_field == 0 {
                    let prev = proto.code[pc - 1];
                    (prev.a() as usize).saturating_sub(a)
                } else {
                    b_field as usize
                };
                let t_raw = bcx.use_var(regs[a]);
                // P11-S5d.D step 3+4 — Float-declared Table operand
                // bitcast to I64; see SetTable for the rationale.
                let t = if matches!(
                    reg_kinds.get(a).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), t_raw)
                } else {
                    t_raw
                };
                // P11-S5d.H — load slab.ptr (= avals base) and asize,
                // compute `atags_ptr = avals_ptr + asize * 8` once for
                // the whole literal store.
                let avals_ptr = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ARRAY_PTR_OFFSET as i32,
                );
                let asize = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let three_imm = bcx.ins().iconst(types::I64, 3);
                let avals_bytes = bcx.ins().ishl(asize, three_imm);
                let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
                for i in 0..b {
                    let src = a + 1 + i;
                    let v = bcx.use_var(regs[src]);
                    // P11-S5d.C — per-PC kind from `current_kinds`,
                    // not the global `reg_kinds`. R[A+i] may legitimately
                    // hold an Int at one SetList PC and a Table at
                    // another (the binary_trees `make` pattern).
                    let kind = current_kinds.get(src).copied().unwrap_or(RegKind::Int);
                    // P11-S5d.D step 3+4 — collapse to I64 first
                    // (lossless when declared F64), then pick the
                    // tag. Handles all (declared × active) ∈ {F64,
                    // I64} × {Int, Float, Table} correctly: a
                    // Float-declared slot whose active kind here is
                    // Int or Table still stores its 8-byte payload
                    // verbatim under the right tag.
                    let is_nil_src = current_is_nil.get(src).copied().unwrap_or(false);
                    let (tag, bits) = if is_nil_src {
                        // P11-S5d.G — slot was last written by LoadNil
                        // in this BB; store the Nil tag + 0 bits so
                        // `t[i] = nil`. Without this an `if t[i] ==
                        // nil` check would see `Int(0)` and miscompile.
                        let zero = bcx.ins().iconst(types::I64, 0);
                        (RAW_TAG_NIL, zero)
                    } else {
                        let bits = if matches!(
                            reg_kinds.get(src).copied().unwrap_or(RegKind::Int),
                            RegKind::Float
                        ) {
                            bcx.ins().bitcast(types::I64, MemFlags::new(), v)
                        } else {
                            v
                        };
                        let tag = match kind {
                            RegKind::Int | RegKind::Unset => RAW_TAG_INT,
                            RegKind::Float => RAW_TAG_FLOAT,
                            RegKind::Table => RAW_TAG_TABLE,
                        };
                        (tag, bits)
                    };
                    let idx_const = bcx.ins().iconst(types::I64, i as i64);
                    let tag_dst = bcx.ins().iadd(atags_ptr, idx_const);
                    let tag_byte = bcx.ins().iconst(types::I8, tag);
                    bcx.ins().store(MemFlags::trusted(), tag_byte, tag_dst, 0);
                    let val_off = bcx.ins().iconst(types::I64, (i as i64) * 8);
                    let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                    bcx.ins().store(MemFlags::trusted(), bits, val_dst, 0);
                }
            }
            Op::GetI => {
                // P11-S5c — `R[A] = R[B][imm(C)]`. P11-S5d.K — inline
                // aget fast path: when the immediate `C` key fits the
                // array part AND the table has no metatable, load the
                // raw 8-byte payload from `array_ptr[key-1] * 8`
                // directly. Mirrors S5c.C's inline aset shape:
                //   if (key - 1) as u64 < asize AND metatable.is_none()
                //     avals_ptr = load array_ptr
                //     bits = load i64 at avals_ptr + (key - 1) * 8
                //     def R[A] = bits
                //   else
                //     bits = luna_jit_table_get_int(t, key)
                // The slow path covers out-of-bounds keys (hash part)
                // and metatable'd tables (helper sets pending_err →
                // dispatcher deopts to interp).
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let t_raw = bcx.use_var(regs[b]);
                let t = if matches!(
                    reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), t_raw)
                } else {
                    t_raw
                };
                let key_imm = ins.c() as i64;

                let asize = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let key_minus_1 = bcx.ins().iconst(types::I64, key_imm - 1);
                let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
                let metatable = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_METATABLE_OFFSET as i32,
                );
                let zero_i64 = bcx.ins().iconst(types::I64, 0);
                let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
                let fast_ok = bcx.ins().band(in_range, no_meta);

                let fast_blk = bcx.create_block();
                let slow_blk = bcx.create_block();
                let merge_blk = bcx.create_block();
                bcx.append_block_param(merge_blk, types::I64);
                bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

                bcx.switch_to_block(fast_blk);
                bcx.seal_block(fast_blk);
                let avals_ptr = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ARRAY_PTR_OFFSET as i32,
                );
                let three = bcx.ins().iconst(types::I64, 3);
                let val_off = bcx.ins().ishl(key_minus_1, three);
                let val_addr = bcx.ins().iadd(avals_ptr, val_off);
                let fast_bits = bcx.ins().load(types::I64, MemFlags::trusted(), val_addr, 0);
                bcx.ins().jump(merge_blk, &[BlockArg::Value(fast_bits)]);

                bcx.switch_to_block(slow_blk);
                bcx.seal_block(slow_blk);
                let key = bcx.ins().iconst(types::I64, key_imm);
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_table_get_int", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call_inst = bcx.ins().call(r, &[t, key]);
                let slow_bits = bcx.inst_results(call_inst)[0];
                bcx.ins().jump(merge_blk, &[BlockArg::Value(slow_bits)]);

                bcx.switch_to_block(merge_blk);
                bcx.seal_block(merge_blk);
                let v = bcx.block_params(merge_blk)[0];
                aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                current_kinds[a] = reg_kinds[a];
                current_is_nil[a] = false;
            }
            Op::GetTable => {
                // P11-S5d.E' / S5d.L — `R[A] = R[B][R[C]]`. Same fast
                // path shape as the GetI inline aget (S5d.K), but the
                // key sits in a register rather than as an immediate.
                // Float keys (5.1/5.2 `t[1.0]`) get an exactness check
                // (fcvt_to_sint + fcvt_from_sint == original) before
                // the bounds + metatable guards; non-exact / fractional
                // keys fall through to the helper which walks the
                // hash part. Int keys (5.3+) skip the fcvt round-trip.
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let c = ins.c() as usize;
                let t_raw = bcx.use_var(regs[b]);
                let t = if matches!(
                    reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), t_raw)
                } else {
                    t_raw
                };
                let key_raw = bcx.use_var(regs[c]);
                let key_kind = a_kind(&reg_kinds, c as u32);
                let is_float_key = matches!(key_kind, RegKind::Float);

                // Compute (key_i64, exact_or_int_key) where exact_or_int_key
                // is the fast-path eligibility flag for the key's
                // numeric form.
                let (key_i64, key_ok) = if is_float_key {
                    let key_int = bcx.ins().fcvt_to_sint(types::I64, key_raw);
                    let key_back = bcx.ins().fcvt_from_sint(types::F64, key_int);
                    let exact = bcx.ins().fcmp(FloatCC::Equal, key_raw, key_back);
                    (key_int, exact)
                } else {
                    // Int key — always "exact" by construction.
                    let always = bcx.ins().iconst(types::I8, 1);
                    (key_raw, always)
                };

                let asize = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let one = bcx.ins().iconst(types::I64, 1);
                let key_minus_1 = bcx.ins().isub(key_i64, one);
                let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
                let metatable = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_METATABLE_OFFSET as i32,
                );
                let zero_i64 = bcx.ins().iconst(types::I64, 0);
                let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
                let bounds_ok = bcx.ins().band(in_range, no_meta);
                let fast_ok = bcx.ins().band(bounds_ok, key_ok);

                let fast_blk = bcx.create_block();
                let slow_blk = bcx.create_block();
                let merge_blk = bcx.create_block();
                bcx.append_block_param(merge_blk, types::I64);
                bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

                bcx.switch_to_block(fast_blk);
                bcx.seal_block(fast_blk);
                let avals_ptr = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    t,
                    TABLE_ARRAY_PTR_OFFSET as i32,
                );
                let three = bcx.ins().iconst(types::I64, 3);
                let val_off = bcx.ins().ishl(key_minus_1, three);
                let val_addr = bcx.ins().iadd(avals_ptr, val_off);
                let fast_bits = bcx.ins().load(types::I64, MemFlags::trusted(), val_addr, 0);
                bcx.ins().jump(merge_blk, &[BlockArg::Value(fast_bits)]);

                bcx.switch_to_block(slow_blk);
                bcx.seal_block(slow_blk);
                let (helper_name, key_arg) = if is_float_key {
                    let key_bits = bcx.ins().bitcast(types::I64, MemFlags::new(), key_raw);
                    ("luna_jit_table_get_float", key_bits)
                } else {
                    ("luna_jit_table_get_int", key_raw)
                };
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function(helper_name, Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call_inst = bcx.ins().call(r, &[t, key_arg]);
                let slow_bits = bcx.inst_results(call_inst)[0];
                bcx.ins().jump(merge_blk, &[BlockArg::Value(slow_bits)]);

                bcx.switch_to_block(merge_blk);
                bcx.seal_block(merge_blk);
                let v = bcx.block_params(merge_blk)[0];
                aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                current_kinds[a] = reg_kinds[a];
                current_is_nil[a] = false;
            }
            Op::Len => {
                // P11-S5c — `R[A] = #R[B]`.
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let t_raw = bcx.use_var(regs[b]);
                // P11-S5d.D step 3+4 — Float-declared table operand
                // bitcast to I64; see SetTable.
                let t = if matches!(
                    reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlags::new(), t_raw)
                } else {
                    t_raw
                };
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                let id = module
                    .declare_function("luna_jit_table_len", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                let call_inst = bcx.ins().call(r, &[t]);
                let v = bcx.inst_results(call_inst)[0];
                aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                current_kinds[a] = RegKind::Int;
            }
            _ => return None,
        }
        pc += 1;
    }
    if !terminated {
        let zero = bcx.ins().iconst(types::I64, 0);
        bcx.ins().return_(&[zero]);
    }
    bcx.seal_all_blocks();
    bcx.finalize();

    module.define_function(fn_id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);

    // v1.3 Phase AOT Stage 3 — diag of the lowered chunk's shape
    // (used to live with the JIT finalize step; moved alongside in
    // the runtime wrapper [`try_compile_int_chunk`]). The generic
    // body only emits the function; finalize is the caller's job.
    let _ = ret_kind; // tracked for diag in the JIT wrapper; backend-agnostic here.

    Some((
        fn_id,
        ChunkMeta {
            num_args: num_params as u8,
            returns_one: sees_return1,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        },
    ))
}

/// S3 — align a value with the Variable's declared Cranelift type
/// before def_var. The scan should have pinned every register's kind
/// tightly; this acts as a safety net so a slipped Unset register
/// (rare, e.g. a register whose only writer is on a path the BFS
/// didn't visit because of a Call wall) doesn't trip the
/// "declared type mismatch" verifier. Real type errors still bail
/// upstream — bitcast i64↔f64 is well-defined for any bit-pattern.
#[inline]
fn aligned_def(
    bcx: &mut FunctionBuilder<'_>,
    regs: &[Variable],
    kinds: &[RegKind],
    idx: usize,
    value: Value,
) {
    let want = match kinds.get(idx).copied().unwrap_or(RegKind::Unset) {
        RegKind::Float => types::F64,
        RegKind::Int | RegKind::Unset | RegKind::Table => types::I64,
    };
    let got = bcx.func.dfg.value_type(value);
    let aligned = if got == want {
        value
    } else {
        bcx.ins().bitcast(want, MemFlags::new(), value)
    };
    bcx.def_var(regs[idx], aligned);
}

/// P11-S5b — try to recognize the 4-op `<env>.math.<fn>(R[arg])` window
/// starting at `start_pc`. Returns `Some(MathFold)` on match, `None`
/// otherwise. Pure inspection — no side effects, no whitelist
/// promotion. Caller (`try_compile_int_chunk`'s pre-scan) marks the
/// participating PCs in `folded_math[]` and pushes the fold to
/// `math_folds`.
fn try_match_math_fold(proto: &Proto, start_pc: usize) -> Option<MathFold> {
    let code = &proto.code;
    let i0 = *code.get(start_pc)?;
    let i1 = *code.get(start_pc + 1)?;
    let i2 = *code.get(start_pc + 2)?;
    let i3 = *code.get(start_pc + 3)?;

    if !matches!(i0.op(), Op::GetTabUp) {
        return None;
    }
    if !matches!(i1.op(), Op::GetField) {
        return None;
    }
    if !matches!(i2.op(), Op::Move) {
        return None;
    }
    if !matches!(i3.op(), Op::Call) {
        return None;
    }

    let a = i0.a();
    // GetTabUp reads upvals[B] indexed by consts[C]. We pin B=0
    // (env upvalue). The frontend invariant (`env_upval_present`
    // check in `try_compile_int_chunk`) guarantees upvals[0].name
    // == "_ENV".
    if i0.b() != 0 {
        return None;
    }
    let k_math = proto.consts.get(i0.c() as usize).copied()?;
    let LuaValue::Str(s) = k_math else {
        return None;
    };
    if s.as_bytes() != b"math" {
        return None;
    }

    // GetField R[A] = R[A].<key>. Same dest as source — the GetTabUp
    // result is consumed in place.
    if i1.a() != a || i1.b() != a {
        return None;
    }
    let k_fn = proto.consts.get(i1.c() as usize).copied()?;
    let LuaValue::Str(fname) = k_fn else {
        return None;
    };
    let fn_name = MATH_LIBM_FNS
        .iter()
        .find_map(|&(needle, name)| (needle == fname.as_bytes()).then_some(name))?;

    // Move R[A+1] = R[arg]. The destination must be the Call's arg
    // slot.
    if i2.a() != a + 1 {
        return None;
    }
    let arg_reg = i2.b();

    // Call R[A], B=2 (1 arg), C=2 (1 return).
    if i3.a() != a || i3.b() != 2 || i3.c() != 2 {
        return None;
    }

    Some(MathFold {
        start_pc,
        fn_name,
        arg_reg,
        dst_reg: a,
    })
}

#[inline]
fn jmp_target(pc: usize, inst: Inst) -> usize {
    // PUC `Jmp`: pc += sJ (after the Jmp is advanced past). New PC =
    // (pc + 1) + sj. Cast carefully — backward jumps would underflow
    // a plain usize add but our forward-only whitelist keeps them out.
    let new_pc = pc as i64 + 1 + inst.sj() as i64;
    new_pc as usize
}

/// Owns the JIT module + holds the entry fn ptr alive for the
/// lifetime of the executable mmap. Drop deallocates the mmap.
///
/// v2.0 Track J sub-step J-D — `_module` is typed as
/// [`SendJitModule`] (J-A's sleeve newtype) so the module's
/// `Send` story stays type-system-asserted at this field. The
/// wrapper is a `#[repr(Rust)]` newtype with `Deref<Target = JITModule>`
/// + `DerefMut`, so existing call sites that touched
/// `handle._module.<method>` keep working transparently. The wrapper
/// also gates `Send` for any future container that wants to hold a
/// `JitHandle`; today the handle itself stays `!Send` because
/// `entry_raw: *const u8` is `!Send`, but the module sleeve is the
/// J-A/J-E join point.
pub struct JitHandle {
    _module: SendJitModule,
    entry_raw: *const u8,
    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    /// Picks the right `extern "C"` fn-type to transmute to at the
    /// call site.
    num_args: u8,
    /// True when the Lua chunk this fn was lowered from contains a
    /// `Return1`; false when only `Return0` is present. Drives the
    /// S2 dispatch wrap (Int wrap vs empty Vec).
    returns_one: bool,
    /// P11-S3 — bit `i = 1` ↔ arg slot `i` is f64 (passed as i64
    /// bit-pattern across the ABI, bitcast inside the JIT). Bits
    /// ≥ MAX_JIT_ARITY are always zero.
    arg_float_mask: u8,
    /// P11-S5d — bit `i = 1` ↔ arg slot `i` is `Gc<Table>` raw ptr.
    /// Mutually exclusive with `arg_float_mask` for the same bit.
    arg_table_mask: u8,
    /// P11-S3 — true iff the Proto's `Return1` value is f64.
    /// Meaningful only when `returns_one == true`.
    ret_is_float: bool,
    /// P11-S5d — true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. Mutually exclusive with `ret_is_float`.
    ret_is_table: bool,
}

// v2.0 Track J sub-step J-E — sibling of the always-on
// `unsafe impl Send for TraceHandle` at `trace.rs:2506`. JitHandle
// holds the same shape: a `SendJitModule` (Send via J-A's wrapper,
// see `send_jit_module.rs`) plus an `entry_raw: *const u8` raw
// fn pointer addressing mcode owned by `_module`. The raw pointer
// is `!Send` by default — this manual impl is the explicit lift.
//
// SAFETY: each field is safely Send:
//   - `_module: SendJitModule` — Send via J-A's `unsafe impl Send
//     for SendJitModule` (`send_jit_module.rs:65`). luna only
//     constructs `JITModule` with `SystemMemoryProvider` (Send,
//     per cranelift-jit's `memory/system.rs:126`).
//   - `entry_raw: *const u8` — addresses mcode in `_module`'s
//     mmap'd page. Because `_module` ships with the handle (the
//     handle owns it by-value), the pointer remains
//     dereferenceable on whichever thread the handle lands on.
//     Read-only on the hot path (transmuted to `extern "C"` fn,
//     called). No aliasing.
//   - remaining fields are primitive scalars.
//
// Cross-thread dispatch is gated separately on the J-D
// `scoped_jit_vm_rebind` RAII (per-`enter_jit` TLS install +
// restore), which works on any OS thread because the TLS slot is
// captured-and-restored at function scope rather than statically
// pinned. Track J-E ship doc:
// `.dev/rfcs/v2.0-track-j-e-verdict.md`.
unsafe impl Send for JitHandle {}

impl JitHandle {
    /// Invoke the entry with zero args. Panics in debug if the
    /// compiled Proto had `num_args > 0`.
    #[inline]
    pub fn call(&self) -> i64 {
        debug_assert_eq!(
            self.num_args, 0,
            "JitHandle::call() is the zero-arg form; use call_with for higher arity"
        );
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        let f: IntChunkFn = unsafe { std::mem::transmute(self.entry_raw) };
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        unsafe { f() }
    }

    /// Invoke the entry with a slice of i64 args. Length must match
    /// `num_args`; the dispatcher picks the right `extern "C"` fn
    /// shape and transmutes at the call site.
    pub fn call_with(&self, args: &[i64]) -> i64 {
        debug_assert_eq!(args.len(), self.num_args as usize);
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        unsafe {
            match self.num_args {
                0 => (std::mem::transmute::<*const u8, IntChunkFn>(self.entry_raw))(),
                1 => (std::mem::transmute::<*const u8, IntFn1>(self.entry_raw))(args[0]),
                2 => (std::mem::transmute::<*const u8, IntFn2>(self.entry_raw))(args[0], args[1]),
                3 => (std::mem::transmute::<*const u8, IntFn3>(self.entry_raw))(
                    args[0], args[1], args[2],
                ),
                4 => (std::mem::transmute::<*const u8, IntFn4>(self.entry_raw))(
                    args[0], args[1], args[2], args[3],
                ),
                _ => unreachable!("MAX_JIT_ARITY enforces num_args <= 4"),
            }
        }
    }

    /// Raw entry fn ptr. S2 stashes a copy in `Proto.jit` so the
    /// dispatch hot-path doesn't have to borrow back through the
    /// handle on every call. The handle itself stays parked in
    /// `Vm.jit_handles` to keep the mmap alive.
    #[inline]
    pub fn entry_raw(&self) -> *const u8 {
        self.entry_raw
    }

    /// v2.0 Track J sub-step J-D — `#[doc(hidden)]` accessor returning
    /// the parked `_module` borrowed at the `SendJitModule` newtype.
    /// Lets the J-D regression test
    /// (`tests/j_d_scoped_rebind_and_sleeve.rs`) statically assert the
    /// field type is the J-A sleeve. The borrow checker enforces the
    /// type match at this fn's signature — if `_module` ever degrades
    /// to bare `JITModule` again, this signature stops compiling.
    #[doc(hidden)]
    #[inline]
    pub fn __j_d_module(&self) -> &SendJitModule {
        &self._module
    }

    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    #[inline]
    pub fn num_args(&self) -> u8 {
        self.num_args
    }

    /// True when the Lua chunk this fn was lowered from ends in
    /// `Return1` (so its result is a single Lua value). False
    /// means the chunk only side-effects + `Return0`; the dispatch
    /// layer should hand the host an empty `Vec<Value>`.
    #[inline]
    pub fn returns_one(&self) -> bool {
        self.returns_one
    }

    /// P11-S3 — packed Float-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is f64 (the dispatcher passes `f64::to_bits` packed into the
    /// i64 ABI slot).
    #[inline]
    pub fn arg_float_mask(&self) -> u8 {
        self.arg_float_mask
    }

    /// P11-S3 — true iff the Proto's `Return1` value is f64. The
    /// dispatcher wraps the i64 ABI return as `Value::Float(
    /// f64::from_bits(r))` when set, `Value::Int(r)` otherwise.
    #[inline]
    pub fn ret_is_float(&self) -> bool {
        self.ret_is_float
    }

    /// P11-S5d — packed Table-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is `Gc<Table>` (the dispatcher passes the raw `as_ptr() as
    /// i64` value).
    #[inline]
    pub fn arg_table_mask(&self) -> u8 {
        self.arg_table_mask
    }

    /// P11-S5d — true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. The dispatcher wraps the i64 ABI return
    /// as `Value::Table(Gc::from_ptr(r as *mut Table))`.
    #[inline]
    pub fn ret_is_table(&self) -> bool {
        self.ret_is_table
    }
}

#[cfg(test)]
mod smoke {
    use cranelift::prelude::*;
    use cranelift_codegen::ir::UserFuncName;
    use cranelift_frontend::FunctionBuilderContext;
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{Linkage, Module};

    /// S0 smoke (carried forward): hand-build recursive
    /// `fib(n: i64) -> i64` directly in cranelift IR, mmap-execute,
    /// and assert fib(28) == 317811.
    #[test]
    fn cranelift_jit_fib28_returns_317811() {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").unwrap();
        flag_builder.set("is_pic", "false").unwrap();
        flag_builder.set("opt_level", "speed").unwrap();
        let isa = cranelift_native::builder()
            .expect("host isa builder")
            .finish(settings::Flags::new(flag_builder))
            .unwrap();
        let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let mut module = JITModule::new(builder);

        let mut sig = module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        let fib_id = module
            .declare_function("fib", Linkage::Local, &sig)
            .expect("declare fib");

        let mut ctx = module.make_context();
        ctx.func.signature = sig.clone();
        ctx.func.name = UserFuncName::user(0, fib_id.as_u32());

        let mut fbc = FunctionBuilderContext::new();
        let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);

        let entry = bcx.create_block();
        let then_blk = bcx.create_block();
        let else_blk = bcx.create_block();
        bcx.append_block_params_for_function_params(entry);
        let n = bcx.block_params(entry)[0];
        bcx.switch_to_block(entry);
        bcx.seal_block(entry);

        let two = bcx.ins().iconst(types::I64, 2);
        let cmp = bcx.ins().icmp(IntCC::SignedLessThan, n, two);
        bcx.ins().brif(cmp, then_blk, &[], else_blk, &[]);

        bcx.switch_to_block(then_blk);
        bcx.seal_block(then_blk);
        bcx.ins().return_(&[n]);

        bcx.switch_to_block(else_blk);
        bcx.seal_block(else_blk);
        let one = bcx.ins().iconst(types::I64, 1);
        let n_minus_1 = bcx.ins().isub(n, one);
        let n_minus_2 = bcx.ins().isub(n, two);
        let fib_ref = module.declare_func_in_func(fib_id, bcx.func);
        let call1 = bcx.ins().call(fib_ref, &[n_minus_1]);
        let r1 = bcx.inst_results(call1)[0];
        let call2 = bcx.ins().call(fib_ref, &[n_minus_2]);
        let r2 = bcx.inst_results(call2)[0];
        let sum = bcx.ins().iadd(r1, r2);
        bcx.ins().return_(&[sum]);

        bcx.finalize();
        module.define_function(fib_id, &mut ctx).expect("define");
        module.clear_context(&mut ctx);
        module.finalize_definitions().expect("finalize");

        let fib_ptr = module.get_finalized_function(fib_id);
        let fib_fn: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fib_ptr) };

        assert_eq!(fib_fn(0), 0);
        assert_eq!(fib_fn(1), 1);
        assert_eq!(fib_fn(10), 55);
        assert_eq!(fib_fn(28), 317811, "fib(28)");
    }
}

#[cfg(test)]
mod s1 {
    use super::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn jit_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let handle = try_compile_int_chunk(cl.proto, false, false)
            .expect("S1 lowerer should accept this chunk");
        handle.call()
    }

    fn interp_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn return_const_int() {
        assert_eq!(jit_int("return 42"), 42);
    }

    #[test]
    fn const_folded_arith() {
        assert_eq!(jit_int("return 1 + 2 + 3"), 6);
    }

    #[test]
    fn locals_add_runtime() {
        assert_eq!(jit_int("local a = 5; local b = 7; return a + b"), 12);
    }

    #[test]
    fn mul_then_add_runtime() {
        assert_eq!(jit_int("local a = 5; return a * a + 1"), 26);
    }

    #[test]
    fn matches_interpreter() {
        for src in [
            "return 0",
            "return 42",
            "return -7",
            "return 1 + 2",
            "local a = 5; local b = 7; return a + b",
            "local a = 5; return a * a + 1",
            "local x = 100; return x - 1",
        ] {
            assert_eq!(jit_int(src), interp_int(src), "src = {src}");
        }
    }

    #[test]
    fn bails_out_on_unsupported_op() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(b"return 'hello'", b"=t").unwrap();
        assert!(try_compile_int_chunk(cl.proto, false, false).is_none());
    }
}

#[cfg(test)]
mod s2 {
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn eval_int_chunk_goes_through_jit() {
        assert_eq!(eval_int("return 42"), 42);
    }

    #[test]
    fn eval_local_arith_goes_through_jit() {
        assert_eq!(eval_int("local a = 5; local b = 7; return a + b"), 12);
        assert_eq!(eval_int("local a = 5; return a * a + 1"), 26);
    }

    #[test]
    fn second_call_hits_cached_native() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm
            .load(b"local a = 5; local b = 7; return a + b", b"=t")
            .expect("compile");
        let v1 = vm.call_value(Value::Closure(cl), &[]).unwrap();
        let v2 = vm.call_value(Value::Closure(cl), &[]).unwrap();
        assert!(matches!(v1.first(), Some(Value::Int(12))));
        assert!(matches!(v2.first(), Some(Value::Int(12))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "one compiled Proto"
        );
    }

    #[test]
    fn unsupported_chunk_falls_back_cleanly() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval("return 'hello'").unwrap();
        match v.first() {
            Some(Value::Str(s)) => assert_eq!(s.as_bytes(), b"hello"),
            other => panic!("expected 'hello', got {other:?}"),
        }
    }

    #[test]
    fn args_disable_jit_path() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(b"local a = 5; return a + 1", b"=t").unwrap();
        let v = vm
            .call_value(Value::Closure(cl), &[Value::Int(99)])
            .unwrap();
        assert!(matches!(v.first(), Some(Value::Int(6))));
    }
}

#[cfg(test)]
mod s2b {
    //! S2b — block-structured lowering with conditional + unconditional
    //! branches. Lt / Le / Eq + Jmp pair into a cranelift `brif`.

    use super::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn jit_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let handle = try_compile_int_chunk(cl.proto, false, false)
            .expect("S2b lowerer should accept this chunk");
        handle.call()
    }

    fn interp_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    fn parity(src: &str) {
        assert_eq!(jit_int(src), interp_int(src), "src = {src}");
    }

    #[test]
    fn if_lt_true_branch() {
        parity("local x = 2; if x < 3 then return 1 else return 0 end");
    }

    #[test]
    fn if_lt_false_branch() {
        parity("local x = 5; if x < 3 then return 1 else return 0 end");
    }

    #[test]
    fn if_le_boundary() {
        for src in [
            "local x = 3; if x <= 3 then return 1 else return 0 end",
            "local x = 4; if x <= 3 then return 1 else return 0 end",
            "local x = 2; if x <= 3 then return 1 else return 0 end",
        ] {
            parity(src);
        }
    }

    #[test]
    fn if_eq() {
        parity("local x = 5; if x == 5 then return 1 else return 0 end");
        parity("local x = 5; if x == 4 then return 1 else return 0 end");
    }

    #[test]
    fn if_no_else() {
        parity("local x = 5; if x < 3 then return 1 end; return 0");
        parity("local x = 2; if x < 3 then return 1 end; return 0");
    }

    #[test]
    fn nested_if_else() {
        parity(
            "local x = 5; local y = 7; \
             if x < 10 then \
               if y < 5 then return 1 else return 2 end \
             else return 3 end",
        );
        parity(
            "local x = 5; local y = 3; \
             if x < 10 then \
               if y < 5 then return 1 else return 2 end \
             else return 3 end",
        );
        parity(
            "local x = 15; \
             if x < 10 then return 1 else return 3 end",
        );
    }

    #[test]
    fn arith_inside_branches() {
        parity(
            "local a = 5; local b = 7; \
             if a < b then return a * b + 1 else return a - b end",
        );
        parity(
            "local a = 50; local b = 7; \
             if a < b then return a * b + 1 else return a - b end",
        );
    }
}

#[cfg(test)]
mod s2c_a {
    //! S2c.A — Protos with `num_params > 0` are JIT-compilable. The
    //! generated `extern "C" fn(...)` takes one i64 per Lua param.
    //! Tested by directly transmuting the raw entry ptr (the
    //! interpreter-side dispatch wire is S2c.B).

    use super::{IntFn1, IntFn2, try_compile_int_chunk};
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    /// Keep the `Vm` alive across the body so the GC doesn't reap the
    /// inner closure's Proto. Returning the `Gc<Proto>` past the Vm's
    /// scope is UB — and was the original bug in this test module.
    fn with_inner<F: FnOnce(&luna_core::runtime::function::Proto)>(src: &str, f: F) {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile main");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run main");
        let inner = match r.first() {
            Some(&Value::Closure(inner)) => inner,
            other => panic!("expected the chunk to return one closure, got {other:?}"),
        };
        f(&inner.proto);
        drop(vm); // explicit so the borrow checker sees the Vm outlives `f`.
    }

    #[test]
    fn add1_compiles_and_runs() {
        with_inner("local function f(n) return n + 1 end; return f", |proto| {
            let handle = try_compile_int_chunk(
                luna_core::runtime::Gc::from_ptr(proto as *const _ as *mut _),
                false,
                false,
            )
            .expect("S2c.A accepts num_params == 1");
            assert_eq!(handle.num_args(), 1);
            assert!(handle.returns_one());
            let f: IntFn1 = unsafe { std::mem::transmute(handle.entry_raw()) };
            assert_eq!(unsafe { f(41) }, 42);
            assert_eq!(unsafe { f(0) }, 1);
            assert_eq!(unsafe { f(-1) }, 0);
            assert_eq!(handle.call_with(&[100]), 101);
        });
    }

    #[test]
    fn two_param_arith() {
        with_inner(
            "local function f(a, b) return a * b + 1 end; return f",
            |proto| {
                let handle = try_compile_int_chunk(
                    luna_core::runtime::Gc::from_ptr(proto as *const _ as *mut _),
                    false,
                    false,
                )
                .expect("S2c.A accepts num_params == 2");
                assert_eq!(handle.num_args(), 2);
                let f: IntFn2 = unsafe { std::mem::transmute(handle.entry_raw()) };
                assert_eq!(unsafe { f(3, 4) }, 13);
                assert_eq!(handle.call_with(&[5, 6]), 31);
            },
        );
    }

    #[test]
    fn param_with_branch() {
        with_inner(
            "local function clip(n) if n < 0 then return 0 end; return n end; return clip",
            |proto| {
                let handle = try_compile_int_chunk(
                    luna_core::runtime::Gc::from_ptr(proto as *const _ as *mut _),
                    false,
                    false,
                )
                .expect("S2c.A accepts param + branch");
                assert_eq!(handle.num_args(), 1);
                let f: IntFn1 = unsafe { std::mem::transmute(handle.entry_raw()) };
                assert_eq!(unsafe { f(5) }, 5);
                assert_eq!(unsafe { f(-5) }, 0);
                assert_eq!(unsafe { f(0) }, 0);
            },
        );
    }

    #[test]
    fn high_arity_bails() {
        with_inner(
            "local function f(a, b, c, d, e) return a + b + c + d + e end; return f",
            |proto| {
                assert!(
                    try_compile_int_chunk(
                        luna_core::runtime::Gc::from_ptr(proto as *const _ as *mut _),
                        false,
                        false,
                    )
                    .is_none(),
                    "5 params is above MAX_JIT_ARITY (4)"
                );
            },
        );
    }
}

#[cfg(test)]
mod s2c_b {
    //! S2c.B — interpreter `Op::Call` fast path. When the target
    //! closure's Proto is cached as `Compiled { num_args > 0 }`
    //! AND every arg slot is `Value::Int`, `begin_call` skips the
    //! interpreter frame setup and runs the cached native fn
    //! in-place.

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn calls_jit_inner_one_arg() {
        assert_eq!(
            eval_int("local function add1(n) return n + 1 end; return add1(41)"),
            42
        );
    }

    #[test]
    fn calls_jit_inner_two_args() {
        assert_eq!(
            eval_int("local function f(a, b) return a * b + 1 end; return f(5, 7)"),
            36
        );
    }

    #[test]
    fn calls_jit_inner_with_branch() {
        let src = "local function clip(n) if n < 0 then return 0 end; return n end; \
             return clip(-3) + clip(7)";
        assert_eq!(eval_int(src), 7);
    }

    #[test]
    fn multiple_calls_share_cache() {
        // The Proto is compiled once; the second call hits the cached
        // entry without recompiling.
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm
            .eval(
                "local function add1(n) return n + 1 end; \
                 return add1(10) + add1(20) + add1(30)",
            )
            .unwrap();
        // 11 + 21 + 31 = 63
        assert!(matches!(v.first(), Some(Value::Int(63))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "Proto compiled exactly once"
        );
    }

    #[test]
    fn jit_failed_state_falls_through() {
        // String body — lowerer bails; interpreter runs the chunk.
        assert!(
            crate::jit_backend::test_vm_new(LuaVersion::Lua55)
                .eval("local function f(n) return tostring(n) end; return #f(42)")
                .unwrap()
                .first()
                .map(|v| matches!(v, Value::Int(_)))
                .unwrap_or(false),
        );
    }
}

#[cfg(test)]
mod s2c_c {
    //! S2c.C — self-recursion through `Op::GetUpval(0)` + `Op::Call`.
    //! fib is the canonical shape; the lowerer recognises the paired
    //! ops and emits a direct cranelift `call` to the current fn,
    //! sidestepping any actual upvalue load.

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn fib_recursive_small() {
        let fib = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib";
        assert_eq!(
            eval_int(
                &format!("{fib} return fib(0)")
                    .replace("return fib return fib(0)", "; return fib(0)")
            ),
            0,
        );
    }

    #[test]
    fn fib_10_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(10)";
        assert_eq!(eval_int(src), 55);
    }

    #[test]
    fn fib_15_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(15)";
        assert_eq!(eval_int(src), 610);
    }

    #[test]
    fn fib_28_matches_interpreter() {
        // The classic baseline workload — must match the interpreter
        // value exactly. If this is wrong, every BASELINE cell movement
        // is meaningless.
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        assert_eq!(eval_int(src), 317811);
    }

    #[test]
    fn recursive_one_compile_per_proto() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm
            .eval(
                "local function fib(n) \
                   if n < 2 then return n end \
                   return fib(n - 1) + fib(n - 2) \
                 end; return fib(10)",
            )
            .unwrap();
        assert!(matches!(v.first(), Some(Value::Int(55))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "fib's Proto compiled exactly once"
        );
    }
}

#[cfg(test)]
mod s2c_c_perf_check {
    //! Sanity check that fib_28's Proto actually flips to Compiled.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    #[test]
    fn fib28_bench_source_flips_proto_to_compiled() {
        let src = "local function f(n) \
                     if n < 2 then return n end \
                     return f(n - 1) + f(n - 2) \
                   end \
                   return f(28)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).unwrap();
        assert!(matches!(v.first(), Some(Value::Int(317811))));
        assert_eq!(
            crate::jit_backend::cache_entry_count(&vm),
            1,
            "fib's Proto should compile exactly once",
        );
    }
}

#[cfg(test)]
mod s3 {
    //! S3 — Float fast path. Per-register type inference + Float
    //! arith / cmp lowerings + bitcast bookends at the i64 ABI
    //! boundary. fib_28 5.1/5.2 (Float-typed n) now JIT-compiles
    //! end-to-end, matching the 5.3/5.4/5.5 path.

    use super::try_compile_int_chunk;
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_float_55(src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    fn eval_with(version: LuaVersion, src: &str) -> Value {
        let mut vm = crate::jit_backend::test_vm_new(version);
        vm.eval(src)
            .expect("eval")
            .into_iter()
            .next()
            .expect("one value")
    }

    /// `return 1.5` const-folds to LoadK(Float(1.5)) — exercises the
    /// LoadK Float whitelist path.
    #[test]
    fn float_const_return() {
        assert_eq!(eval_float_55("return 1.5"), 1.5);
    }

    /// `return 1.5 + 2.5` const-folds again, but
    /// `local a=1.5; local b=2.5; return a+b` keeps two runtime LoadKs
    /// plus an Add — exercises the Float-arith path.
    #[test]
    fn float_runtime_arith() {
        assert_eq!(
            eval_float_55("local a = 1.5; local b = 2.5; return a + b"),
            4.0,
        );
        assert_eq!(
            eval_float_55("local a = 1.5; local b = 2.5; return a * b"),
            3.75,
        );
    }

    /// `local x=2.5; if x < 3.0 then return 1.0 else return 0.0 end`
    /// — exercises Float cmp (fcmp) + brif branching with Float regs.
    #[test]
    fn float_branch() {
        assert_eq!(
            eval_float_55("local x = 2.5; if x < 3.0 then return 1.0 else return 0.0 end"),
            1.0,
        );
        assert_eq!(
            eval_float_55("local x = 4.5; if x < 3.0 then return 1.0 else return 0.0 end"),
            0.0,
        );
    }

    /// Mixed Int + Float in one register sequence — the sweep should
    /// reject (`unify(Int, Float) → false`) and the chunk runs through
    /// the interpreter. `1 + 0.5` is const-folded by the parser so use
    /// runtime locals.
    #[test]
    fn mixed_int_float_bails_cleanly() {
        // Direct call to try_compile — should return None.
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        // R[0] = LoadI 1; R[1] = LoadK 0.5; Add R[2] = R[0]+R[1]
        let cl = vm
            .load(b"local a = 1; local b = 0.5; return a + b", b"=t")
            .expect("compile");
        // The scan unifies R[0]=Int with R[1]=Float in Add → bail.
        // (Or pins R[2]=Float and back-propagates — the unify rules
        // require both operands to agree, so this returns None.)
        assert!(try_compile_int_chunk(cl.proto, false, false).is_none());
        // And the interpreter still produces the correct result.
        let v = vm
            .eval("local a = 1; local b = 0.5; return a + b")
            .expect("eval");
        assert!(matches!(v.first(), Some(&Value::Float(1.5))));
    }

    /// fib_28 under Lua 5.2 — the inner closure's `n` is Float
    /// (5.2 has no integer subtype), and the JIT must take it via
    /// Float arg + Float ret.
    #[test]
    fn fib28_5_2_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua52, src) {
            Value::Float(f) => assert_eq!(f, 317811.0),
            other => panic!("expected Float(317811.0), got {other:?}"),
        }
    }

    /// fib_28 under Lua 5.1 — extra wrinkle: the inner closure has
    /// 2 upvals (`_ENV` placeholder at slot 0, `fib` self at slot 1).
    /// The scanner's `self_upval_idx` tracker should pin slot 1 from
    /// the first GetUpval(b=1) and lower the recursion correctly.
    #[test]
    fn fib28_5_1_matches_interpreter() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua51, src) {
            Value::Float(f) => assert_eq!(f, 317811.0),
            other => panic!("expected Float(317811.0), got {other:?}"),
        }
    }

    /// Sanity guard: 5.5 fib still goes through the Int path. The
    /// JIT cache slot ends up with `arg_float_mask: 0,
    /// ret_is_float: false` and the result is `Value::Int(317811)`.
    #[test]
    fn fib28_5_5_still_int() {
        let src = "local function fib(n) \
                     if n < 2 then return n end \
                     return fib(n - 1) + fib(n - 2) \
                   end; return fib(28)";
        match eval_with(LuaVersion::Lua55, src) {
            Value::Int(i) => assert_eq!(i, 317811),
            other => panic!("expected Int(317811), got {other:?}"),
        }
    }

    /// Cache-key correctness regression: two protos with identical
    /// bytecode shape (`LoadK k0 + Return1`) but different `consts[0]`
    /// must not share a slot. Before S3 added `proto.consts` to the
    /// hash, the second proto inherited the first's compiled constant.
    #[test]
    fn cache_key_includes_consts() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v1 = vm.eval("return 1.5").unwrap();
        assert!(matches!(v1.first(), Some(&Value::Float(f)) if f == 1.5));
        let v2 = vm.eval("return 2.5").unwrap();
        assert!(matches!(v2.first(), Some(&Value::Float(f)) if f == 2.5));
        // Two distinct compiled protos, two cache entries.
        assert_eq!(crate::jit_backend::cache_entry_count(&vm), 2);
    }

    /// 5.4 Division `a / b` always yields a Float in PUC semantics.
    /// `local a = 3.0; local b = 2.0; return a / b` should compile
    /// and produce 1.5.
    #[test]
    fn float_div() {
        assert_eq!(
            eval_float_55("local a = 3.0; local b = 2.0; return a / b"),
            1.5,
        );
    }
}

#[cfg(test)]
mod s5a {
    //! S5a — `ForPrep` / `ForLoop` whitelist for Lua 5.4+ Int loops.
    //! `loop_int_1m` cells under 5.4 / 5.5 now compile to a Cranelift
    //! counted loop. Pre-5.3 dialects continue through the interpreter
    //! (S5a.B target); Float loops continue through the interpreter
    //! (S5a.C target).

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int_55(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn for_1_to_1000_sums_to_500500() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 1000 do s = s + i end return s"),
            500500,
        );
    }

    #[test]
    fn for_descending_step_minus_1() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 10, 1, -1 do s = s + i end return s"),
            55,
        );
    }

    #[test]
    fn for_empty_loop_skips_body() {
        // `for i = 10, 1` with positive default step is an empty range.
        assert_eq!(
            eval_int_55("local s = 0 for i = 10, 1 do s = s + 1 end return s"),
            0,
        );
    }

    #[test]
    fn for_single_iter() {
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 1 do s = s + i end return s"),
            1,
        );
    }

    #[test]
    fn for_body_references_control_var() {
        // R[A+3] is the body-visible `i`. Body reads it via Move/Add
        // — must match interpreter.
        assert_eq!(
            eval_int_55("local s = 0 for i = 1, 100 do s = s + i * 2 end return s"),
            10100,
        );
    }

    /// The headline cell — `for i = 1, 1000000` uses `LoadK Int(1000000)`
    /// for the limit (not LoadI, since 1000000 > MAX_SBX). Both the
    /// LoadK Int whitelist extension and ForPrep/ForLoop have to be in
    /// place for this to compile.
    #[test]
    fn loop_int_1m_5_5_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_55(src), 500000500000);
    }

    /// 5.4 mirrors 5.5 — both `post53`, same emit path, shares the
    /// thread-local cache slot.
    #[test]
    fn loop_int_1m_5_4_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_with(LuaVersion::Lua54, src), 500000500000);
    }

    /// Pre-5.3 dialects use the pre-decrement ForPrep form, which S5a
    /// doesn't handle. The chunk still has to run correctly through
    /// the interpreter and yield the same answer.
    #[test]
    fn loop_int_1k_pre53_runs_through_interpreter() {
        let src = "local s = 0 for i = 1, 1000 do s = s + i end return s";
        // 5.1 / 5.2 numeric `for` uses Floats; the loop variable is a
        // Float there, so the interpreter returns Float(500500.0).
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua51);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => assert_eq!(f, 500500.0),
            other => panic!("expected Float(500500.0) under 5.1, got {other:?}"),
        }
        // 5.3 uses Ints like 5.4/5.5, but the pre53 ForPrep form bails
        // the JIT — interpreter still returns Int(500500).
        assert_eq!(eval_int_with(LuaVersion::Lua53, src), 500500);
    }

    /// Cache-key correctness: same source loaded as 5.4 (post53) and
    /// 5.3 (pre53) lands in distinct cache slots — each compiles to a
    /// different form (count form vs pre-decrement form). The dialect
    /// bit in `proto_cache_key` is what keeps these from sharing a
    /// slot. (Before S5a.B, the 5.3 slot would be `Failed` for the
    /// same reason; after S5a.B both forms are emitted, so the assert
    /// shape is "two distinct Compiled slots, both with the right
    /// loop semantics".)
    #[test]
    fn cache_pre53_post53_distinct() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 100 do s = s + i end return s";

        let mut vm55 = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl55 = vm55.load(src, b"=t").expect("compile");
        let r55 = vm55.call_value(Value::Closure(cl55), &[]).expect("run");
        assert!(matches!(
            cl55.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
        assert!(matches!(r55.first(), Some(&Value::Int(5050))));

        let mut vm53 = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let cl53 = vm53.load(src, b"=t").expect("compile");
        let r53 = vm53.call_value(Value::Closure(cl53), &[]).expect("run");
        assert!(matches!(
            cl53.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
        assert!(matches!(r53.first(), Some(&Value::Int(5050))));

        // v2.0 Track J sub-step J-B Phase D — cache is per-`Vm` now,
        // so each Vm carries exactly one entry for its own dialect.
        // Pre-J-B this asserted `cache_entry_count() == 2` over the
        // thread-local cache (both Vms shared the cross-Vm cache and
        // their distinct dialect bit produced two entries). The
        // dialect-distinguishing invariant under test is preserved by
        // asserting each Vm cached its own version exactly once.
        assert_eq!(crate::jit_backend::cache_entry_count(&vm55), 1);
        assert_eq!(crate::jit_backend::cache_entry_count(&vm53), 1);
    }

    /// A non-immediate step bails out — variable `local step = 2;
    /// for i = 1, N, step` puts `step` in a register written by
    /// LoadI then read by ForPrep, but a step that comes from a
    /// non-LoadI source can't be const-folded at JIT time. Verify the
    /// chunk still produces the right answer through the interpreter.
    #[test]
    fn non_immediate_step_runs_through_interpreter() {
        // `for i = 1, 10, s` where s is a variable reachable only as
        // a Move target — the scan should not see a `LoadI` for the
        // step register and bail.
        let src = "local function f(s) local sum = 0 for i = 1, 10, s do sum = sum + i end return sum end; return f(2)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => assert_eq!(i, 25), // 1+3+5+7+9
            other => panic!("expected Int(25), got {other:?}"),
        }
    }
}

#[cfg(test)]
mod s5a_b {
    //! S5a.B — `ForPrep` / `ForLoop` pre-5.3 (limit-compare) form.
    //! Lua 5.3 `for i = 1, N` chunks now JIT-compile under the
    //! pre-decrement ForPrep + limit-compare ForLoop emit. 5.1 / 5.2
    //! still go through the interpreter because their loop variable
    //! lives in a Float register (S5a.C target).

    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_int_53(src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    #[test]
    fn pre53_for_1_to_1000_sums_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 1, 1000 do s = s + i end return s"),
            500500,
        );
    }

    #[test]
    fn pre53_descending_step_minus_1_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 10, 1, -1 do s = s + i end return s"),
            55,
        );
    }

    #[test]
    fn pre53_empty_loop_skips_body_under_5_3() {
        // `for i = 10, 1` with default positive step is empty: ForPrep
        // pre-decrements R[A] = 10 - 1 = 9; ForLoop's first add yields
        // 10, which is > limit=1 → exit without entering body.
        assert_eq!(
            eval_int_53("local s = 0 for i = 10, 1 do s = s + 1 end return s"),
            0,
        );
    }

    #[test]
    fn pre53_single_iter_under_5_3() {
        assert_eq!(
            eval_int_53("local s = 0 for i = 5, 5 do s = s + i end return s"),
            5,
        );
    }

    /// The 5.3 headline cell — `for i = 1, 1000000` with LoadK Int(1000000).
    #[test]
    fn loop_int_1m_5_3_matches_interpreter() {
        let src = "local s = 0 for i = 1, 1000000 do s = s + i end return s";
        assert_eq!(eval_int_53(src), 500000500000);
    }

    /// Pin the JIT state so a future change that silently regresses
    /// this back to the interpreter is caught.
    #[test]
    fn loop_int_1m_5_3_jit_state_compiled() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua53);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(500000500000))));
    }
}

#[cfg(test)]
mod s5a_c {
    //! S5a.C — `ForPrep` / `ForLoop` Float form (pre53 + post53).
    //!
    //! Lua 5.1 / 5.2 have no Int subtype, so numeric `for i = 1, N`
    //! lowers to a Float-typed loop var (R[A] = LoadF 1, R[A+1] =
    //! LoadK Float(N) or LoadF, R[A+2] = LoadI step). S5a.C extends the
    //! scanner to pick the loop kind from R[A]'s scanned kind and adds
    //! Float emit branches to ForPrep / ForLoop.
    //!
    //! The Float ForLoop has the same shape for pre53 and post53 (Lua's
    //! Float branch never had a count form). The Float ForPrep splits
    //! by dialect: pre53 pre-decrement + unconditional jump, post53
    //! empty-test + state-set + fall through.
    //!
    //! Body arithmetic on Float locals was already covered by S3, so
    //! `s = s + i` inside the body lowers to fadd against the visible
    //! R[A+3] register.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    fn eval_float_with(version: LuaVersion, src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    /// post53 (5.5) explicit Float loop — `for i = 1.0, 1000.0 do …`.
    #[test]
    fn post53_float_for_1_to_1000_sums_under_5_5() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua55,
                "local s = 0.0 for i = 1.0, 1000.0 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// pre53 (5.3) explicit Float loop — same source, pre-decrement
    /// ForPrep + Float ForLoop.
    #[test]
    fn pre53_float_for_1_to_1000_sums_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 0.0 for i = 1.0, 1000.0 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// 5.1 headline cell — `for i = 1, 1000000` lowers to LoadF init +
    /// LoadK Float(1e6) limit + LoadI step. With S5a.C the chunk
    /// JIT-compiles end-to-end.
    #[test]
    fn loop_int_1m_5_1_matches_interpreter() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua51,
                "local s = 0 for i = 1, 1000000 do s = s + i end return s",
            ),
            500000500000.0,
        );
    }

    /// 5.2 headline cell — same source.
    #[test]
    fn loop_int_1m_5_2_matches_interpreter() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua52,
                "local s = 0 for i = 1, 1000000 do s = s + i end return s",
            ),
            500000500000.0,
        );
    }

    /// Pin JitProtoState for the 5.1 headline cell — confirms the
    /// chunk actually hits the JIT (not just the interpreter fallback).
    #[test]
    fn loop_int_1m_5_1_jit_state_compiled() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua51);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Float(f)) if f == 500000500000.0));
    }

    /// Regression guard — 5.5 still goes through the Int path even with
    /// the Float branches in place. R[A] stays Int (LoadI init), so the
    /// scan still pins Int.
    #[test]
    fn loop_int_1m_5_5_still_jit_int_path() {
        use luna_core::runtime::function::JitProtoState;
        let src = b"local s = 0 for i = 1, 1000000 do s = s + i end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(500000500000))));
    }

    /// pre53 Float descending step. `for i = 1000.0, 1.0, -1 do …`
    /// exercises the negative-step path of the Float ForPrep / ForLoop.
    #[test]
    fn pre53_float_descending_step_minus_1_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 0.0 for i = 1000.0, 1.0, -1 do s = s + i end return s",
            ),
            500500.0,
        );
    }

    /// pre53 Float empty loop — `for i = 10.0, 1.0` with default
    /// positive step. The pre-decrement ForPrep writes R[A] = 9.0; the
    /// first ForLoop add yields 10.0, which is > limit=1.0 → exit
    /// without entering body. Accumulator stays at its initial value.
    #[test]
    fn pre53_float_empty_loop_skips_body_under_5_3() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua53,
                "local s = 7.0 for i = 10.0, 1.0 do s = s + 1.0 end return s",
            ),
            7.0,
        );
    }

    /// post53 Float empty loop — explicit empty test (init > limit for
    /// positive step) jumps straight to exit.
    #[test]
    fn post53_float_empty_loop_skips_body_under_5_5() {
        assert_eq!(
            eval_float_with(
                LuaVersion::Lua55,
                "local s = 7.0 for i = 10.0, 1.0 do s = s + 1.0 end return s",
            ),
            7.0,
        );
    }
}

#[cfg(test)]
mod s5b {
    //! S5b — `math.<fn>(arg)` libcall fold.
    //!
    //! Recognized 4-op windows (`GetTabUp _ENV "math"` → `GetField R[A]
    //! "<fn>"` → `Move R[A+1] R[arg]` → `Call R[A] B=2 C=2`) collapse
    //! into a single cranelift `call` to libm. Bytecode is
    //! dialect-invariant: the same window appears across 5.1 – 5.5.
    //! Loop kind (Int vs Float) varies per dialect — S5a.C handles the
    //! loop, and S5b's emit converts an Int loop var to f64 at the
    //! libm call boundary via `fcvt_from_sint`.
    //!
    //! Correctness baseline for each cell is the interpreter's exact
    //! `Value::Float` return. We compare with `f64::EPSILON`-scaled
    //! tolerance because libm sin/cos and the interpreter's own libm
    //! calls share the same C runtime — bit-exact equality is the
    //! expected outcome on macOS / Linux, but a 1-ULP slack guards
    //! against future cross-platform drift.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_float_with(version: LuaVersion, src: &str) -> f64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Float(f)) => f,
            other => panic!("expected float return, got {other:?}"),
        }
    }

    fn interp_only_float_with(version: LuaVersion, src: &str) -> f64 {
        // Bypass the JIT cache so we have an interpreter-only reference
        // result to compare against. The chunk runs through Vm::eval
        // exactly the same; we just disable cache hits by clearing
        // before AND we drop into call_value via load → ensuring the
        // JIT path is what gets hit. The reference is computed by
        // hand-mirroring the loop in Rust at the call sites below.
        let _ = version;
        let _ = src;
        unreachable!("references are precomputed in each test")
    }

    /// Headline cell: `math.sin(i)` over an Int loop in 5.5. Pin the
    /// JIT state to `Compiled` so we know the fold took, then assert
    /// the result matches `f64::sin` over the same integer range.
    #[test]
    fn math_sin_5_5_matches_libm() {
        let _ = interp_only_float_with;
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.sin(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).sin()).sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),
            "math.sin sum mismatch: jit={r_jit} ref={r_ref}"
        );
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
    }

    /// Symmetric `math.cos(i)` check — exercises the second entry in
    /// `MATH_LIBM_FNS` and the const-bytes cache-key extension (sin
    /// and cos chunks no longer collide).
    #[test]
    fn math_cos_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).cos()).sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),
            "math.cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
    }

    /// Two folds in one body: `math.sin(i) * math.cos(i)`. The cos
    /// fold writes back into a register that the sin fold's
    /// `Move` temp also targeted; S5b's RegKind handler skips the
    /// Move's unification on folded PCs so the conflicting kinds
    /// (Int from Move-of-loop-var, Float from cos result) don't
    /// abort compile.
    #[test]
    fn math_sin_cos_product_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 1000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=1000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 32.0).max(1e-9),
            "sin*cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
    }

    /// Headline `math_loop_100k` cell at full N=100 000 — confirms
    /// the actual bench source compiles and returns within libm
    /// tolerance on 5.5.
    #[test]
    fn math_loop_100k_5_5_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!(
            (r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),
            "100k sin*cos sum mismatch: jit={r_jit} ref={r_ref}"
        );
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
    }

    /// 5.4 — same body, Int loop var. Confirms the post53 Int loop +
    /// math fold combination compiles.
    #[test]
    fn math_loop_100k_5_4_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua54, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.3 — pre53 Int loop + math fold. Cache-key `pre53` bit
    /// distinguishes from 5.5's slot.
    #[test]
    fn math_loop_100k_5_3_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua53, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.2 — Float loop (`LoadF init / LoadK Float(N) limit`) + math
    /// fold. The arg conversion path here is identity (loop var is
    /// already Float).
    #[test]
    fn math_loop_100k_5_2_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua52, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// 5.1 — same Float-loop path as 5.2. Both share the pre53 +
    /// Float fork in the ForPrep/ForLoop scanner.
    #[test]
    fn math_loop_100k_5_1_matches_libm() {
        let src =
            "local s = 0.0 for i = 1, 100000 do s = s + math.sin(i) * math.cos(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua51, src);
        let r_ref: f64 = (1..=100_000)
            .map(|i| (i as f64).sin() * (i as f64).cos())
            .sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 1024.0).max(1e-6),);
    }

    /// Cache key uses string-byte content (not just discriminant) —
    /// `math.sin` and `math.cos` chunks land in distinct cache slots
    /// even though their bytecode shape is identical bar the `C`
    /// operand of GetField.
    ///
    /// v2.0 Track J sub-step J-B Phase D — refactored to one Vm
    /// (cache is per-`Vm` now). The invariant under test (distinct
    /// libcall name → distinct slot) is preserved by asserting the
    /// cache grew from 1 (after sin) to 2 (after cos) in the same Vm.
    #[test]
    fn math_libcall_distinct_fns_distinct_cache() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let _ = vm
            .eval("local s = 0.0 for i = 1, 4 do s = s + math.sin(i) end return s")
            .expect("sin eval");
        let n_after_sin = crate::jit_backend::cache_entry_count(&vm);
        let _ = vm
            .eval("local s = 0.0 for i = 1, 4 do s = s + math.cos(i) end return s")
            .expect("cos eval");
        let n_after_cos = crate::jit_backend::cache_entry_count(&vm);
        assert!(
            n_after_cos > n_after_sin,
            "sin/cos chunks must hash to distinct cache slots (sin={n_after_sin} cos={n_after_cos})"
        );
    }

    /// `math.sqrt(i)` exercises another libm fn from the supported
    /// set. Result matches `f64::sqrt`.
    #[test]
    fn math_sqrt_5_5_matches_libm() {
        let src = "local s = 0.0 for i = 1, 100 do s = s + math.sqrt(i) end return s";
        let r_jit = eval_float_with(LuaVersion::Lua55, src);
        let r_ref: f64 = (1..=100).map(|i| (i as f64).sqrt()).sum();
        assert!((r_jit - r_ref).abs() <= (r_ref.abs() * f64::EPSILON * 16.0).max(1e-12),);
    }

    /// Unsupported math fn (`math.pi` access, no Call) bails. A bare
    /// `math.pi + i` body has no `Call` after the `GetField`, so the
    /// fold pre-scan rejects it and the chunk falls back to interp.
    #[test]
    fn math_constant_access_bails_to_interp() {
        let src = "local s = 0.0 for i = 1, 4 do s = s + math.pi end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
        match r.first() {
            Some(&Value::Float(f)) => {
                assert!((f - 4.0 * std::f64::consts::PI).abs() < 1e-12);
            }
            other => panic!("expected float, got {other:?}"),
        }
    }

    /// Two-arg math fn (`math.atan(y, x)`) bails — the `Call B=2 C=2`
    /// gate requires exactly 1 arg + 1 result. With two args the Call
    /// has B=3, the fold pre-scan rejects it.
    #[test]
    fn math_two_arg_atan_bails() {
        let src = "local s = 0.0 for i = 1, 4 do s = s + math.atan(i, 2) end return s";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
    }
}

#[cfg(test)]
mod s5c {
    //! S5c — `NewTable` / `SetTable` / `GetI` / `Len` JIT via Rust
    //! helpers. The dispatcher pins the active `Vm` in the
    //! `JIT_VM` thread-local; cranelift `Linkage::Import` calls
    //! land in `luna_jit_new_table` / `_table_set_int` /
    //! `_table_set_float_float` / `_table_get_int` / `_table_len`
    //! which demote the pinned ptr to `&mut Vm` for the duration of
    //! one helper-level operation.
    //!
    //! Headline cell: `table_alloc_10k` 5.3 / 5.4 / 5.5 (Int-loop
    //! dialects). 5.1 / 5.2 use a Float loop var that conflicts
    //! with the `Len` result's Int kind on the same register —
    //! documented bail; revisit in S5c.B if the cell needs it.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    /// Small `table_alloc` — proves the NewTable + SetTable + Len
    /// path runs end-to-end on the active Vm.
    #[test]
    fn table_alloc_10_matches_interpreter_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 10 do t[i] = i end return #t",
            ),
            10,
        );
    }

    /// Headline cell at full N=10 000. Pinned `JitProtoState`.
    #[test]
    fn table_alloc_10k_5_5_jit_state_compiled() {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// 5.4 dialect — same shape as 5.5 (Int loop var, Int-typed
    /// SetTable values).
    #[test]
    fn table_alloc_10k_5_4_matches_interpreter() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua54,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// 5.3 dialect — pre53 ForPrep + Int loop var.
    #[test]
    fn table_alloc_10k_5_3_matches_interpreter() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// `t[50]` after building — exercises `Op::GetI` with an
    /// immediate Int key. Stores `i * 2`, so the read at index 50
    /// must come back as 100.
    #[test]
    fn table_get_int_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 100 do t[i] = i * 2 end return t[50]",
            ),
            100,
        );
    }

    /// `#t` returns Int — confirms `Op::Len` JIT path against the
    /// interpreter's `len()`.
    #[test]
    fn table_len_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 42 do t[i] = i end return #t",
            ),
            42,
        );
    }

    /// P11-S5d.F — Float loop var + Int `Len` result re-using the
    /// same register slot now JIT-compiles. The `Len` scan no
    /// longer force-unifies R[A] with `Int`; instead it leaves the
    /// declared kind alone when it's already Float/Int (and pins
    /// Int only when Unset). The helper's i64 return goes through
    /// `aligned_def`'s I64↔F64 bitcast on the writer side, and the
    /// `Return1` emit bitcasts F64→I64 (using the declared Float
    /// kind) so the i64 length ferries through unchanged. The
    /// `latest_writer_kind[a] = Int` assignment ensures `ret_kind`
    /// reflects Len's Int even when the declared slot stays Float.
    #[test]
    fn table_alloc_10k_5_2_jit_compiles() {
        table_alloc_10k_jit_compiles_for_version(LuaVersion::Lua52);
    }

    /// P11-S5d.F — symmetric to the 5.2 case: 5.1's frontend lowers
    /// `for i = 1, 10000` with a Float loop var (no Int subtype), so
    /// the same Float/Int slot reuse at `#t` post-loop applies.
    #[test]
    fn table_alloc_10k_5_1_jit_compiles() {
        table_alloc_10k_jit_compiles_for_version(LuaVersion::Lua51);
    }

    fn table_alloc_10k_jit_compiles_for_version(ver: LuaVersion) {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(
            matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} table_alloc Proto did not JIT-compile (state: {:?})",
            cl.proto.jit.get()
        );
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// Non-Int key (string) bails. The whitelist's SetTable arm
    /// requires R[B] to be Int/Float (numeric loop var Move).
    #[test]
    fn table_with_string_key_bails() {
        let src = "local t = {} t['a'] = 1 return t['a']";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let _ = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Failed));
    }

    /// NewTable with a presized array (`b > 0`) now compiles via
    /// S5d.B (the `NewTable.B` field feeds
    /// `luna_jit_new_table_sized`, and SetList builds the literal
    /// inline). The chunk loads `{10, 20, 30}` then reads `t[2]`;
    /// both ops are whitelisted.
    #[test]
    fn presized_newtable_now_jits() {
        let src = "local t = {10, 20, 30} return t[2]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(20))));
    }
}

#[cfg(test)]
mod s5c_b {
    //! S5c.B — `NewTable` presize fold. When the bytecode opens a
    //! counted `for i = 1, N do … end` window immediately after
    //! a `local t = {}`, the JIT emits
    //! `luna_jit_new_table_sized(N)` instead of the plain
    //! `luna_jit_new_table()`. The pre-sized array part skips
    //! every intermediate `rehash` round that the iteration body
    //! would otherwise trigger via `Table::set_int`'s `insert_new`
    //! path.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    fn eval_int_with(version: LuaVersion, src: &str) -> i64 {
        let mut vm = crate::jit_backend::test_vm_new(version);
        let v = vm.eval(src).expect("eval");
        match v.first() {
            Some(&Value::Int(i)) => i,
            other => panic!("expected int return, got {other:?}"),
        }
    }

    /// Headline cell — `table_alloc_10k 5.5` JIT-compiles and
    /// returns the correct length after pre-sized fill.
    #[test]
    fn table_alloc_10k_5_5_presized() {
        let src = "local t = {} for i = 1, 10000 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(10000))));
    }

    /// 5.4 — same shape (Int loop var). Presize hint extracted
    /// from the LoadI limit window.
    #[test]
    fn table_alloc_10k_5_4_presized() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua54,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// 5.3 — pre53 ForPrep form, same Int loop var. The presize
    /// scan inspects PCs prep_pc-4..prep_pc and is dialect-
    /// agnostic.
    #[test]
    fn table_alloc_10k_5_3_presized() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {} for i = 1, 10000 do t[i] = i end return #t",
            ),
            10000,
        );
    }

    /// Compile-time `limit` from a `LoadK Int` (the 10 000 fits
    /// in `sbx`, but a larger limit exercises the `LoadK` arm).
    #[test]
    fn table_alloc_loadk_limit_5_5() {
        let src = "local t = {} for i = 1, 65536 do t[i] = i end return #t";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src.as_bytes(), b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(matches!(cl.proto.jit.get(), JitProtoState::Compiled { .. }));
        assert!(matches!(r.first(), Some(&Value::Int(65536))));
    }

    /// Small `N=4` — verify the fold doesn't misfire on tiny
    /// tables. Result and JIT state both pin.
    #[test]
    fn table_alloc_4_small_presize() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 4 do t[i] = i end return #t",
            ),
            4,
        );
    }

    /// `for i = 1, N, 2 do …` — step ≠ 1. The presize map skips
    /// this entry; the chunk still compiles (S5c path) but uses
    /// the non-sized helper. Correctness unaffected.
    #[test]
    fn step_ne_1_falls_back_to_empty_helper() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {} for i = 1, 10, 2 do t[i] = i end return #t",
            ),
            // Indices 1, 3, 5, 7, 9 → t[1..9] filled at odd slots
            // only. `#t` returns the largest border, which here is
            // 1 (t[2] is nil so border is 1).
            1,
        );
    }

    /// P11-S5c.C — inline aset writes the **right** payload at the
    /// **right** offset. Sum-of-cubes is sensitive to either a
    /// stride-1 error in `key_minus_1 * 8` (would corrupt avals
    /// indexing) or a misaligned atag write (interp would read back
    /// a Nil tag and treat the value as Nil → 0). Both modes would
    /// fail the assertion; the only way to hit `36` is correct.
    #[test]
    fn inline_aset_payload_round_trip_5_5() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua55,
                "local t = {}
                 for i = 1, 5 do t[i] = i * i end
                 return t[1] + t[2] + t[3] + t[4] + t[5]",
            ),
            // 1 + 4 + 9 + 16 + 25 == 55
            55,
        );
    }

    /// Cross-dialect: 5.3 reads the same values back. 5.3 uses the
    /// pre53 ForPrep form, so this exercises a different emit
    /// branch than 5.5 while still hitting the inline aset path.
    #[test]
    fn inline_aset_payload_round_trip_5_3() {
        assert_eq!(
            eval_int_with(
                LuaVersion::Lua53,
                "local t = {}
                 for i = 1, 5 do t[i] = i * i end
                 return t[1] + t[2] + t[3] + t[4] + t[5]",
            ),
            55,
        );
    }
}

#[cfg(test)]
mod s5d_a {
    //! S5d.A — ABI extension: `arg_table_mask` + `ret_is_table`.
    //! Threads `Value::Table` through the JIT entry as a raw
    //! `Gc<Table>` ptr and back. No new ops yet — this commit only
    //! lifts the S5c bail on Table-typed params and adds the
    //! dispatcher path. The follow-up sub-step (S5d.B) adds
    //! NewTable b>0 + SetList so binary_trees' `make` Proto can
    //! actually compile.
    use luna_core::runtime::Value;
    use luna_core::version::LuaVersion;

    /// `function f(t) return t[1] end` — Table param + Int return.
    /// JIT path: param marshalled as Gc ptr, GetI reads array slot,
    /// Return1 sends Int back. Verifies the ABI plumbing without
    /// any of S5d.B's NewTable/SetList plumbing.
    #[test]
    fn table_param_int_return_round_trip_5_5() {
        use luna_core::runtime::function::JitProtoState;
        // Build f's Proto via a chunk that returns the function so we
        // can poke its JIT state. The outer chunk bails (Op::Closure
        // isn't whitelisted); the inner Proto is what we're after.
        let src = b"local function f(t) return t[1] end return f";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let inner = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // The inner Proto's JIT state is `Untried` until first call.
        // Drive it by calling f(t) and confirming the Compiled state +
        // correct result.
        let t_chunk = b"local t = {} t[1] = 42 return t";
        let t_cl = vm.load(t_chunk, b"=u").expect("compile");
        let t_v = vm.call_value(Value::Closure(t_cl), &[]).expect("run");
        let tv = *t_v.first().expect("t");
        let r2 = vm
            .call_value(Value::Closure(inner), &[tv])
            .expect("call f(t)");
        assert!(matches!(r2.first(), Some(&Value::Int(42))));
        assert!(matches!(
            inner.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
    }

    /// `function f(t) return #t end` — same shape, Len op.
    #[test]
    fn table_param_len_5_5() {
        let src = "local function f(t) return #t end
                   local t = {} t[1] = 1 t[2] = 1 t[3] = 1 return f(t)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(3))));
    }
}

#[cfg(test)]
mod s5d_b {
    //! S5d.B — `NewTable b > 0` (presize hint from the bytecode
    //! field) + `Op::SetList` (fixed-count `{a, b, c}` literals) +
    //! BB-level `defines_table` dataflow (replaces the S5c blanket
    //! "has_conditional && has_new_table → bail" gate with a sound
    //! intersection-at-joins must-defined analysis).
    //!
    //! The BB dataflow accepts patterns like `make`'s two-branch
    //! structure — both branches independently `NewTable + SetList`
    //! into the same register before Return1 — while still rejecting
    //! the unsound false-branch-only-define case the linear forward
    //! walk would let through.
    //!
    //! Per-register RegKind across branches is still single-kind:
    //! a register that's `Int` in BB-then and `Table` in BB-else
    //! still bails. The full make / check Protos hit this; S5d.C
    //! is the BB-level kind tracking that unblocks them.
    use luna_core::runtime::Value;
    use luna_core::runtime::function::JitProtoState;
    use luna_core::version::LuaVersion;

    /// Simple SetList literal — `{1, 2, 3}` in a fn body. NewTable
    /// b=3 + LoadI×3 + SetList b=3 + Return1.
    #[test]
    fn newtable_b3_setlist_int_5_5() {
        let src = b"local function f() return {10, 20, 30} end return f";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let cl = vm.load(src, b"=t").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let inner = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // First call populates / drives JIT.
        let r2 = vm.call_value(Value::Closure(inner), &[]).expect("call f()");
        // f returns a Table; assert ret_is_table threaded through.
        let t = match r2.first() {
            Some(&Value::Table(t)) => t,
            other => panic!("expected table, got {other:?}"),
        };
        assert_eq!(t.len(), 3);
        assert!(matches!(t.get_int(2), Value::Int(20)));
        assert!(matches!(
            inner.proto.jit.get(),
            JitProtoState::Compiled { .. }
        ));
    }

    /// Read a fixed-N table — exercises GetI through a Table param
    /// in concert with the SetList that built it.
    #[test]
    fn setlist_then_geti_round_trip_5_5() {
        let src = "local function get(t, i) return t[i] end
                   local function make() return {7, 11, 13} end
                   return get(make(), 2)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(11))));
    }

    /// Both branches independently `NewTable + SetList` into R[A]
    /// before `Return1` — proves the BB-level dataflow accepts
    /// what S5c's blanket gate would have blocked. The function
    /// param is an Int so we don't hit the RegKind reuse conflict
    /// the binary_trees `make` Proto carries (that's S5d.C).
    #[test]
    fn conditional_both_branches_new_table_5_5() {
        let src = "local function f(flag)
                     if flag == 1 then return {1, 2}
                     else return {3, 4} end
                   end
                   return f(1)[2] + f(0)[1]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // f(1) -> {1,2}, [2] = 2. f(0) -> {3,4}, [1] = 3. Sum = 5.
        assert!(matches!(r.first(), Some(&Value::Int(5))));
    }

    /// S5d.C — the binary_trees `make` Proto now JIT-compiles:
    /// the Int-to-Table re-use on R[1] (LoadI 0 for an Eq compare,
    /// then `NewTable` for the table) is allowed via the relaxed
    /// `RegKind::unify`; `latest_writer_kind` carries the per-PC
    /// kind so `Return1` correctly wraps as `Value::Table`. The
    /// variadic `Op::Call C=0` + `Op::SetList B=0` pattern in the
    /// else branch resolves to `count = A_call - A_list` at scan.
    /// End-to-end `make(3)` builds the same 8-leaf tree the
    /// interpreter would.
    #[test]
    fn make_proto_5_5_round_trip() {
        let src = "local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   local t = make(3)
                   return t[1][1][1][1] + t[2][2][2][2]";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        assert!(matches!(r.first(), Some(&Value::Int(2))));
    }

    /// Full binary_trees bench source round-trip — make + check
    /// both JIT'd, sum across 16 trees of depth 10 matches interp.
    #[test]
    fn binary_trees_n10_round_trip_5_5() {
        let src = "local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local sum = 0
                   for i = 1, 16 do sum = sum + check(make(10)) end
                   return sum";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // Each depth-10 tree has 2^11 - 1 == 2047 internal nodes;
        // check returns 2047 per tree; 16 * 2047 == 32752.
        assert!(matches!(r.first(), Some(&Value::Int(32752))));
    }

    /// S5d.D step 3+4 — 5.1/5.2 binary_trees `make` Proto JIT-
    /// compiles. The frontend uses `LoadF R[1]=0` for the `if d
    /// == 0` Eq compare in one BB and `NewTable R[1]` for the
    /// returned table in another, so R[1] sees Float+Table on
    /// disjoint paths. S5d.D's relaxed `unify(Float, Table)` lets
    /// the scan keep R[1] declared in whichever shape the first
    /// writer pinned; emit-side `use_var` callers for Table
    /// operands bitcast F64→I64 when the slot is Float-declared.
    fn make_proto_jit_compiles_for_version(ver: LuaVersion) {
        let src = b"local function make(d)
                     if d == 0 then return {1, 1}
                     else return {make(d-1), make(d-1)} end
                   end
                   return make";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=make").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let make_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // Drive a few calls to warm the JIT cache.
        for d in 0..3 {
            vm.call_value(Value::Closure(make_cl), &[Value::Int(d)])
                .expect("call make(d)");
        }
        assert!(
            matches!(make_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} make Proto did not JIT-compile (state: {:?})",
            make_cl.proto.jit.get()
        );
    }

    #[test]
    fn make_proto_jit_compiles_5_1() {
        make_proto_jit_compiles_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn make_proto_jit_compiles_5_2() {
        make_proto_jit_compiles_for_version(LuaVersion::Lua52);
    }

    /// P11-S5d.G — `binary_trees`' cross_dialect harness uses
    /// `{nil, nil}` as the leaf node. LoadNil + SetList must
    /// JIT-compile so `make` stops bailing across all dialects.
    fn make_nil_proto_jit_compiles_for_version(ver: LuaVersion) {
        let src = b"local function make(d)
                     if d == 0 then return {nil, nil}
                     else return {make(d-1), make(d-1)} end
                   end
                   return make";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=make").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let make_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        for d in 0..3 {
            vm.call_value(Value::Closure(make_cl), &[Value::Int(d)])
                .expect("call make(d)");
        }
        assert!(
            matches!(make_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} make {{nil,nil}} Proto did not JIT-compile (state: {:?})",
            make_cl.proto.jit.get()
        );
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_1() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_2() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua52);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_3() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua53);
    }

    #[test]
    fn make_nil_proto_jit_compiles_5_5() {
        make_nil_proto_jit_compiles_for_version(LuaVersion::Lua55);
    }

    /// P11-S5d.G — `return nil` must NOT JIT into `Value::Int(0)`
    /// (the dispatcher's ret_is_float=false default would wrap the
    /// i64 helper return as `Int`, masking the Nil). The Return1
    /// scan bails on a LoadNil source so the interp returns the
    /// real Nil.
    #[test]
    fn return_nil_does_not_miscompile_5_5() {
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm
            .eval("local function f() return nil end return f()")
            .expect("eval");
        assert!(
            matches!(r.first(), Some(&Value::Nil)),
            "expected Nil, got {r:?}"
        );
    }

    /// binary_trees' `check` Proto JIT-compiles end-to-end: GetI
    /// through a Table param, Int Eq + branch, self-recursive
    /// Call returning Int, Int Add, Return1 of an Int. No Table
    /// re-use conflict — all kind unifications converge cleanly.
    #[test]
    fn check_proto_jit_compiles_5_5() {
        let src = "local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local leaf = {1, 1}
                   local node = {leaf, leaf}
                   return check(node)";
        let mut vm = crate::jit_backend::test_vm_new(LuaVersion::Lua55);
        let r = vm.eval(src).expect("eval");
        // node = {leaf, leaf} where leaf is terminal.
        // check(node) hits else: 1 + check(leaf) + check(leaf)
        //                       = 1 + 1 + 1 = 3.
        assert!(matches!(r.first(), Some(&Value::Int(3))));
    }

    /// P11-S5d.E' — a Table-typed param + a single `R[B][R[C]]`
    /// read is the minimal `OP_GETTABLE` shape; it must JIT in
    /// 5.1 / 5.2 (which lower `t[1]` as GetTable + a Float key,
    /// not GetI + an immediate Int). The chunk returns the read
    /// value; the assertion is that the Proto reaches the
    /// Compiled state at all.
    fn get_table_simple_jit_for_version(ver: LuaVersion) {
        let src = b"local function get(t, k) return t[k] end
                   return get";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let cl = vm.load(src, b"=get").expect("compile");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let get_cl = match r.first() {
            Some(&Value::Closure(c)) => c,
            other => panic!("expected closure, got {other:?}"),
        };
        // Warm: drive a couple of calls with a normal (no-metatable)
        // table so the JIT path is reached. The cache lookup happens
        // on first call; subsequent calls run the cached entry.
        let table = vm.heap.new_table();
        let _ = unsafe { table.as_mut() }.set_int(&mut vm.heap, 1, Value::Float(42.0));
        for _ in 0..3 {
            let _ = vm
                .call_value(
                    Value::Closure(get_cl),
                    &[Value::Table(table), Value::Float(1.0)],
                )
                .expect("call get(t, 1.0)");
        }
        assert!(
            matches!(get_cl.proto.jit.get(), JitProtoState::Compiled { .. }),
            "{ver:?} get Proto did not JIT-compile (state: {:?})",
            get_cl.proto.jit.get()
        );
    }

    #[test]
    fn get_table_simple_jit_5_1() {
        get_table_simple_jit_for_version(LuaVersion::Lua51);
    }

    #[test]
    fn get_table_simple_jit_5_2() {
        get_table_simple_jit_for_version(LuaVersion::Lua52);
    }

    /// binary_trees' `check` Proto in 5.1 / 5.2 — same source as the
    /// 5.5 test, but lowering uses `OP_GETTABLE` for `t[1]` / `t[2]`
    /// (no GetI). Reaches `JitProtoState::Compiled` once S5d.E'
    /// whitelists GetTable.
    fn check_proto_jit_compiles_pre53(ver: LuaVersion) {
        let src = "local function check(t)
                     if t[1] == 1 then return 1 end
                     return 1 + check(t[1]) + check(t[2])
                   end
                   local leaf = {1, 1}
                   local node = {leaf, leaf}
                   return check(node)";
        let mut vm = crate::jit_backend::test_vm_new(ver);
        let r = vm.eval(src).expect("eval");
        // 5.1 / 5.2 have no Int subtype — the literal `1` is Float;
        // arith and Return ferry the f64 bits unchanged.
        match r.first() {
            Some(&Value::Float(f)) if (f - 3.0).abs() < 1e-9 => {}
            other => panic!("{ver:?} check(node) expected Float(3.0), got {other:?}"),
        }
    }

    #[test]
    fn check_proto_jit_compiles_5_1() {
        check_proto_jit_compiles_pre53(LuaVersion::Lua51);
    }

    #[test]
    fn check_proto_jit_compiles_5_2() {
        check_proto_jit_compiles_pre53(LuaVersion::Lua52);
    }
}

// v1.1 A1 Session C — Default Cranelift-backed JIT. Moved here from
// `src/jit/abi.rs` (Session A's in-place introduction) because the
// trait impls call into Cranelift-bound free fns
// (`cache_lookup_or_compile`, `enter_jit`,
// `try_compile_trace_with_options`, `last_compile_checkpoint`) that
// can't live in luna-core. luna-core's `Vm::install_jit_backend` is
// how the `luna` crate installs this struct on top of the default
// `NullJitBackend`.

/// Default Cranelift-backed JIT backend. The `luna` crate's
/// `Vm::new_minimal_with_jit` / `install_default_jit` /
/// `luaL_newstate` swap this in via `Vm::install_jit_backend`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CraneliftBackend;

impl IntChunkCompiler for CraneliftBackend {
    // v2.0 Track J sub-step J-B Phases D/E — pass storage through to
    // `cache_lookup_or_compile`; the cache lookup + handle park both
    // operate on `Vm.jit.storage.{cache,cache_handles}`.
    fn try_compile(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        proto: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
        pre53: bool,
        float_only: bool,
    ) -> CompileResult {
        match cache_lookup_or_compile(storage, proto, pre53, float_only) {
            Some((
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            )) => CompileResult::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            },
            None => CompileResult::Skipped,
        }
    }

    #[allow(clippy::not_unsafe_ptr_arg_deref)] // Trait impl required by IntChunkCompiler; SAFETY documented below — caller is the dispatcher with a live `&mut Vm`.
    fn enter(
        &self,
        vm: *mut luna_core::vm::Vm,
        cl: Option<luna_core::runtime::Gc<luna_core::runtime::LuaClosure>>,
    ) -> JitVmGuard {
        // SAFETY: the dispatcher derived `vm` from a live `&mut Vm`
        // and the JIT entry that runs under this guard does not
        // re-enter Rust against `Vm` except through the TLS pointer
        // this call installs (helpers reach Vm via `JIT_VM`). Vm is
        // `?Send` / single-threaded. The raw-ptr indirection here
        // only sidesteps the lexical borrow conflict against
        // `self.chunk_compiler`.
        // SAFETY: called only from Cranelift-emitted JIT code under an active JitVmGuard; the guard guarantees JIT_VM TLS holds a live &mut Vm for the dispatch window.
        let vm_ref: &mut luna_core::vm::Vm = unsafe { &mut *vm };
        enter_jit(vm_ref, cl)
    }
}

impl TraceCompiler for CraneliftBackend {
    // v2.0 Track J sub-step J-B Phase F — pass storage through so
    // `try_compile_trace_with_options` parks the trace's `JITModule`
    // on the per-`Vm` `storage.trace_handles` Vec.
    fn try_compile_trace(
        &self,
        storage: &mut dyn luna_core::jit::JitStorage,
        record: &TraceRecord,
        opts: CompileOptions,
    ) -> Option<CompiledTrace> {
        trace::try_compile_trace_with_options(storage, record, opts)
    }

    fn last_compile_checkpoint(&self) -> &'static str {
        trace::last_compile_checkpoint()
    }
}
