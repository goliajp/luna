//! JIT pipeline (luna crate side; the trait surface and pure
//! data types live in `luna_core::jit`).
//!
//! - Proto → Cranelift IR lowerer for a whitelisted opcode subset.
//! - Dispatch wire: `Vm::call_value` short-circuits to a cached
//!   native fn when the Proto fits the whitelist.
//! - Block-structured lowering with conditional + unconditional
//!   branches; a paired `Lt|Le|Eq` + `Jmp` is lowered as a cranelift
//!   `brif`.
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
use luna_core::jit::{CompileResult, IntChunkCompiler, JitVmGuard, MAX_JIT_ARITY, TraceCompiler};
use luna_core::runtime::Value as LuaValue;
use luna_core::runtime::function::Proto;
use luna_core::runtime::{Gc, LuaStr};
use luna_core::vm::isa::{Inst, Op};

/// per-Lua-register type lattice. `Unset` is the bottom;
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
    /// `Gc<Table>` raw pointer pun. Represented as I64 at the
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
            // Int+Table coexist (both I64-shaped at the
            // Cranelift level; `maybe_table[reg]` + Table-bail on
            // arith/cmp/ForPrep keeps the semantic guard).
            (RegKind::Int, RegKind::Table) | (RegKind::Table, RegKind::Int) => true,
            // Float+Table coexist via I64↔F64
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
            // (Table) in another — both safe per-BB, but a stricter
            // `unify` would reject the slot reuse globally.
            (RegKind::Float, RegKind::Table) | (RegKind::Table, RegKind::Float) => true,
            _ => false,
        }
    }
}

// codegen-bearing modules live here on the luna
// side. `IntChunkFn`, the trait surface (`IntChunkCompiler`,
// `TraceCompiler`, `CompileResult`, `NullJitBackend`), `JitVmGuard`,
// and the pure trace data types moved to `luna_core::jit` so embedders
// who depend on luna-core alone never link Cranelift.
pub mod trace;

// Owner newtype for `cranelift_jit::JITModule`, used by the per-`Vm`
// JIT storage. Scoped `pub(crate)` — no embedder surface.
pub(crate) mod code_memory;
mod const_operands;
mod getupval_roles;
mod math_fold;
mod send_jit_module;
mod trace_backend;
use getupval_roles::determine_getupval_roles;
use math_fold::try_match_math_fold;
#[allow(unused_imports)]
pub use send_jit_module::SendJitModule;

// `JIT_VM` / `JIT_CL` TLS slots, the helper
// extern "C" fns, `enter_jit`, and `scoped_rebind` all moved to
// the sibling `luna-jit-helpers` crate so `luna-jit-llvm`
// (alt backend) can reuse them without dragging Cranelift in.
// Star-re-export preserves every existing `super::luna_jit_*` /
// `crate::jit_backend::*` call path inside this crate.
pub use luna_jit_helpers::*;

// concrete per-`Vm` JIT storage struct
// (cache + cache_handles + trace_handles). Installed alongside the
// `CraneliftBackend` by `crate::install_default_jit`. luna-core sees
// it through the opaque `JitStorage` trait only.
pub(crate) mod storage;

#[cfg(test)]
mod trace_build_tests;

// inline `#[cfg(test)] mod xx { ... }` blocks
// throughout this file call `crate::jit_backend::test_vm_new(version)` / `crate::jit_backend::test_vm_new_minimal(version)`
// and expect the Cranelift backend to be installed. luna-core's
// `Vm::new` defaults to `NullJitBackend`, so these helpers install it.
#[cfg(test)]
fn test_vm_new(version: luna_core::version::LuaVersion) -> luna_core::vm::Vm {
    let mut vm = luna_core::vm::Vm::new(version);
    vm.install_jit_backend(CraneliftBackend, CraneliftBackend);
    // pair the backend install with the
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

mod chunk_cache;
mod chunk_lower;
mod chunk_module;
mod jit_handle;
pub(crate) use chunk_cache::CacheEntry;
pub use chunk_cache::{cache_clear, cache_entry_count, cache_lookup_or_compile};
pub use chunk_lower::lower_int_chunk_into;
pub use chunk_module::try_compile_int_chunk;
pub use jit_handle::JitHandle;

// `IntFn1..4` + `MAX_JIT_ARITY` live in
// `luna_core::jit` so `vm/exec.rs` (in luna-core) can name them
// when transmuting JIT entry pointers. Bumping the arity cap stays
// mechanical: extend the alias list in `luna-core/src/jit/abi.rs`,
// add the matching match arm in `luna-core/src/vm/exec.rs`, then
// add the matching `IntFnN` codegen here.

/// supported `math.<fn>(arg)` libm folds. Each entry is
/// the Lua-side method name (as it appears in `consts` after the
/// `GetField` k-operand) paired with the libm symbol the cranelift
/// `Linkage::Import` resolves to via `dlsym(RTLD_DEFAULT)`. Same
/// signature across all entries — `(f64) -> f64`. Single-arg
/// numerics only; `math.log(x, base)` / `math.atan(y, x)` /
/// `math.max(...)` use a different bytecode window (B≠2) so the
/// pattern matcher rejects them.
///
/// On 5.3+ `floor`/`ceil` return integers ([`math_fold::is_rounding`]) and
/// `atan(y)` is `atan2(y, 1)`, rounded differently from libm `atan`;
/// the emit handles both.
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

/// `Table` layout constants used by the inline-aset
/// fast path. Cranelift IR walks past the helper call ABI by
/// loading the table's array pointer and length directly from the
/// `Gc<Table>` raw ptr, skipping the per-iter thread-local read
/// + `Gc::from_ptr` non-null check + `Table::set_int` dispatch
/// that the helper-call path pays.
///
/// The offsets are computed at compile time via `std::mem::offset_of!`,
/// so the IR follows whatever `Table`'s `#[repr(C)]` layout chooses
/// today. The static asserts below pin the assumptions the IR
/// itself can't verify (`RawVal` packed to 8 bytes, and the `Table.asize` field width).
///
/// Table keeps `array_ptr: *mut u8` as the single
/// source of truth for "where does the array part live?". The pointer
/// targets either the inline storage embedded in the Table struct
/// (asize <= INLINE_ASIZE) or an external slab the table owns. The JIT
/// loads `array_ptr` directly — no branching, no indirection
/// — and computes `atags_ptr = array_ptr + asize * 8` on the fly.
pub(crate) const TABLE_ARRAY_PTR_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, array_ptr);
pub(crate) const TABLE_ASIZE_OFFSET: usize = std::mem::offset_of!(luna_core::runtime::Table, asize);
/// `Option<Gc<Table>>` is 8 bytes via NPO; 0 ⇔ None.
/// Inline aget reads this to short-circuit on metatable.is_none()
/// rather than always going through the helper's metatable check.
pub(crate) const TABLE_METATABLE_OFFSET: usize =
    std::mem::offset_of!(luna_core::runtime::Table, metatable);
pub(crate) const STR_SHORT_OFFSET: usize = luna_core::runtime::string::jit_layout::STR_SHORT_OFFSET;
/// A string's `u32` hash (see `field_slot::emit_str_key_absent`).
pub(crate) const STR_HASH_OFFSET: usize = luna_core::runtime::string::jit_layout::STR_HASH_OFFSET;
/// A node's `i32` link to the next node of its chain (`-1` at the end).
pub(crate) const NODE_NEXT_OFFSET: usize = luna_core::runtime::table::jit_layout::NODE_NEXT_OFFSET;
pub(crate) const TABLE_ACOUNT_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_ACOUNT_OFFSET as i32;
pub(crate) const TABLE_APREFIX_OFFSET: i32 =
    luna_core::runtime::table::jit_layout::TABLE_APREFIX_OFFSET as i32;

/// table-field IC scaffold.
///
/// Byte offset of the hash part's node pointer. luna-core's
/// `runtime::table::jit_layout` module computes this against the live
/// `Table` struct, then we re-export it here so trace.rs can refer to
/// it locally.
#[allow(dead_code)]
pub(crate) const TABLE_NODES_PTR_OFFSET: usize =
    luna_core::runtime::table::jit_layout::TABLE_NODES_OFFSET;
/// The `u32` node mask (node count - 1, `u32::MAX` when empty). The IC's
/// shape-stability guard compares it against the recorder's node count
/// less one.
pub(crate) const TABLE_NODE_MASK_OFFSET: usize =
    luna_core::runtime::table::jit_layout::TABLE_NODE_MASK_OFFSET;
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
    // `asize` is u64 so a single `load i64` yields the
    // array-part length; the JIT then shifts left 3 to multiply by 8
    // for the `atags_ptr = array_ptr + asize * 8` computation.
    assert!(std::mem::size_of::<u64>() == 8);
};

/// a single recognized `math.<fn>(arg)` fold. The four
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
    /// The result is an integer: 5.3+ `floor` / `ceil`.
    int_result: bool,
    /// The `"math"` and `"<fn>"` constant keys, for the entry check that
    /// the field still holds the library function.
    math_key: Gc<LuaStr>,
    name_key: Gc<LuaStr>,
}

impl MathFold {
    fn result_kind(&self) -> RegKind {
        if self.int_result {
            RegKind::Int
        } else {
            RegKind::Float
        }
    }
}

/// backend-agnostic metadata describing one
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

#[cfg(test)]
mod chunk_tests_basic;
#[cfg(test)]
mod chunk_tests_calls;
#[cfg(test)]
mod chunk_tests_loops;
#[cfg(test)]
mod chunk_tests_math;
#[cfg(test)]
mod chunk_tests_setlist;
#[cfg(test)]
mod chunk_tests_tables;

// Default Cranelift-backed JIT. Lives in this crate because the
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
    // pass storage through to
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

    fn enter(
        &self,
        vm: *mut luna_core::vm::Vm,
        cl: Option<luna_core::runtime::Gc<luna_core::runtime::LuaClosure>>,
    ) -> JitVmGuard {
        luna_jit_helpers::enter_jit_ptr(vm, cl)
    }
}
