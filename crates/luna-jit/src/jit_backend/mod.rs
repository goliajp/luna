// CARVE-OUT: pre-existing god file, shrinking on every touch
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
use luna_core::jit::{
    CompileResult, IntChunkCompiler, IntChunkFn, IntFn1, IntFn2, IntFn3, IntFn4, JitVmGuard,
    MAX_JIT_ARITY, TraceCompiler,
};
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
pub(crate) use chunk_cache::CacheEntry;
pub use chunk_cache::{cache_clear, cache_entry_count, cache_lookup_or_compile};

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
const TABLE_ACOUNT_OFFSET: i32 = luna_core::runtime::table::jit_layout::TABLE_ACOUNT_OFFSET as i32;
const TABLE_APREFIX_OFFSET: i32 =
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

/// build a fresh `JITModule` configured with
/// all `luna_jit_*` helper symbols pre-registered. Shared by the
/// runtime JIT entry [`try_compile_int_chunk`] and tests; the AOT
/// pipeline (luna-aot) builds an `ObjectModule` instead and feeds it
/// to the same [`lower_int_chunk_into`] generic body.
fn build_jit_module_with_helpers() -> Option<JITModule> {
    let mut builder =
        JITBuilder::with_isa(method_isa()?, cranelift_module::default_libcall_names());
    builder.memory_provider(Box::new(code_memory::CodeMemory::new()));
    // register the Rust helpers so the cranelift JIT can resolve them at
    // finalize time: executables that link luna as an rlib strip the
    // `#[no_mangle]` symbols, and the default `dlsym(RTLD_DEFAULT)` resolver
    // then fails. The libm symbols the math folds use (`sin`, `cos`, …) are
    // linked from libc and stay resolvable via dlsym. A lookup function
    // rather than one `symbol` entry each: entries are owned strings in a
    // map built anew for every module.
    builder.symbol_lookup_fn(Box::new(method_helper));
    Some(JITModule::new(builder))
}

/// The method JIT's target, built once: the flags never change, and
/// building it per function cost about as much as compiling a small one.
fn method_isa() -> Option<cranelift_codegen::isa::OwnedTargetIsa> {
    static ISA: std::sync::OnceLock<Option<cranelift_codegen::isa::OwnedTargetIsa>> =
        std::sync::OnceLock::new();
    ISA.get_or_init(|| {
        let mut flag_builder = settings::builder();
        flag_builder.set("use_colocated_libcalls", "false").ok();
        flag_builder.set("is_pic", "false").ok();
        flag_builder.set("opt_level", "speed").ok();
        // Release builds leave the IR verifier out, as the trace JIT does
        // (see `build_trace_jit_module`).
        if !cfg!(debug_assertions) {
            flag_builder.set("enable_verifier", "false").ok();
        }
        cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flag_builder))
            .ok()
    })
    .clone()
}

/// The address of a Rust helper the method JIT's code calls.
fn method_helper(name: &str) -> Option<*const u8> {
    Some(match name {
        "luna_jit_new_table" => luna_jit_new_table as *const u8,
        "luna_jit_new_table_sized" => luna_jit_new_table_sized as *const u8,
        "luna_jit_table_set_int" => luna_jit_table_set_int as *const u8,
        "luna_jit_table_set_float_float" => luna_jit_table_set_float_float as *const u8,
        "luna_jit_table_set_raw" => luna_jit_table_set_raw as *const u8,
        "luna_jit_table_get_int" => luna_jit_table_get_int as *const u8,
        "luna_jit_table_get_float" => luna_jit_table_get_float as *const u8,
        "luna_jit_table_len" => luna_jit_table_len as *const u8,
        "luna_jit_upval_get" => luna_jit_upval_get as *const u8,
        "luna_jit_upval_get_float" => luna_jit_upval_get_float as *const u8,
        "luna_jit_self_upval_check" => luna_jit_self_upval_check as *const u8,
        "luna_jit_math_fn_is_library" => luna_jit_math_fn_is_library as *const u8,
        "luna_jit_park_deopt" => luna_jit_park_deopt as *const u8,
        "luna_jit_table_get_int_checked" => luna_jit_table_get_int_checked as *const u8,
        "luna_jit_table_get_float_checked" => luna_jit_table_get_float_checked as *const u8,
        _ => return None,
    })
}

/// Try to JIT-compile `proto`. Returns `None` when any opcode in the
/// body falls outside the cumulative whitelist — the interpreter then
/// handles the chunk unchanged. `pre53` (Lua 5.1 / 5.2 / 5.3) selects
/// the pre-5.3 `ForPrep` / `ForLoop` form; pass `false` (Lua 5.4 /
/// 5.5) for the counted-loop form. The dialect bit also participates
/// in the cache key — see `proto_cache_key`.
///
/// thin wrapper around the backend-agnostic
/// [`lower_int_chunk_into`] generic; constructs a `JITModule`,
/// finalizes the compiled fn into RWX memory, and wraps the entry ptr
/// in a [`JitHandle`] that owns the module for the entry's lifetime.
pub fn try_compile_int_chunk(proto: Gc<Proto>, pre53: bool, float_only: bool) -> Option<JitHandle> {
    let mut module = send_jit_module::UnpublishedModule::new(build_jit_module_with_helpers()?);
    let (fn_id, meta) = lower_int_chunk_into(&mut *module, proto, pre53, float_only)?;
    module.finalize_definitions().ok()?;

    // `LUNA_JIT_TRACE=1` prints one line per
    // successful JIT compile with the Proto's source location +
    // signature. A regression in
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
        // wrap with the `SendJitModule` sleeve
        _module: module.publish(),
        entry_raw: ptr,
        num_args: meta.num_args,
        returns_one: meta.returns_one,
        arg_float_mask: meta.arg_float_mask,
        arg_table_mask: meta.arg_table_mask,
        ret_is_float: meta.ret_is_float,
        ret_is_table: meta.ret_is_table,
    })
}

/// The tag a method-JIT register of `kind` holds, as `Value::unpack`
/// reports it.
fn want_tag(kind: RegKind) -> i64 {
    match kind {
        RegKind::Int | RegKind::Unset => RAW_TAG_INT,
        RegKind::Float => RAW_TAG_FLOAT,
        RegKind::Table => RAW_TAG_TABLE,
    }
}

/// A typed table read, `R[A] = t[key]`, whose register kind was inferred
/// statically: the value's tag is checked against `want`, and a value of
/// another type (nil for a missing key, a string, ...) leaves the compiled
/// call so the interpreter re-runs it, as a metatable does. Reading the
/// raw payload unchecked turned a nil into integer 0 or float 0.0.
///
/// After an inline store of a non-nil value into array slot `idx` whose
/// tag was `old_tag`, keep `Table`'s `acount` / `aprefix` as `aset` does
/// (a nil slot turning non-nil counts, and extends a leading run that ends
/// exactly there), then continue at `next`.
fn emit_array_fill_count(
    bcx: &mut FunctionBuilder,
    t: Value,
    idx: Value,
    old_tag: Value,
    next: Block,
) {
    let count_blk = bcx.create_block();
    let was_nil = bcx.ins().icmp_imm_u(IntCC::Equal, old_tag, RAW_TAG_NIL);
    bcx.ins().brif(was_nil, count_blk, &[], next, &[]);
    bcx.switch_to_block(count_blk);
    bcx.seal_block(count_blk);
    let flags = MemFlagsData::trusted();
    let acount = bcx.ins().load(types::I32, flags, t, TABLE_ACOUNT_OFFSET);
    let acount = bcx.ins().iadd_imm_u(acount, 1);
    bcx.ins().store(flags, acount, t, TABLE_ACOUNT_OFFSET);
    let aprefix = bcx.ins().load(types::I32, flags, t, TABLE_APREFIX_OFFSET);
    let wide = bcx.ins().uextend(types::I64, aprefix);
    let at_end = bcx.ins().icmp(IntCC::Equal, wide, idx);
    let grown = bcx.ins().iadd_imm_u(aprefix, 1);
    let aprefix = bcx.ins().select(at_end, grown, aprefix);
    bcx.ins().store(flags, aprefix, t, TABLE_APREFIX_OFFSET);
    bcx.ins().jump(next, &[]);
}

/// `fast_ok` selects the inline array read (`key - 1` in range, no
/// metatable); otherwise `slow` names a `*_checked` helper and its key.
fn emit_checked_get<M: Module>(
    bcx: &mut FunctionBuilder<'_>,
    module: &mut M,
    t: Value,
    fast_ok: Value,
    key_minus_1: Value,
    slow: (&str, Value),
    want: i64,
) -> Option<Value> {
    let fast_blk = bcx.create_block();
    let slow_blk = bcx.create_block();
    let deopt_blk = bcx.create_block();
    let merge_blk = bcx.create_block();
    bcx.append_block_param(merge_blk, types::I64);
    bcx.ins().brif(fast_ok, fast_blk, &[], slow_blk, &[]);

    // atags trail the avals: the tag of slot i is at avals_ptr + asize * 8 + i
    bcx.switch_to_block(fast_blk);
    bcx.seal_block(fast_blk);
    let avals_ptr = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        TABLE_ARRAY_PTR_OFFSET as i32,
    );
    let asize = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        TABLE_ASIZE_OFFSET as i32,
    );
    let avals_bytes = bcx.ins().ishl_imm_u(asize, 3);
    let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
    let tag_addr = bcx.ins().iadd(atags_ptr, key_minus_1);
    let tag = bcx
        .ins()
        .uload8(types::I64, MemFlagsData::trusted(), tag_addr, 0);
    let tag_ok = bcx.ins().icmp_imm_u(IntCC::Equal, tag, want);
    let val_off = bcx.ins().ishl_imm_u(key_minus_1, 3);
    let val_addr = bcx.ins().iadd(avals_ptr, val_off);
    let fast_bits = bcx
        .ins()
        .load(types::I64, MemFlagsData::trusted(), val_addr, 0);
    bcx.ins().brif(
        tag_ok,
        merge_blk,
        &[BlockArg::Value(fast_bits)],
        deopt_blk,
        &[],
    );

    bcx.switch_to_block(slow_blk);
    bcx.seal_block(slow_blk);
    let (helper, key) = slow;
    let slot = bcx.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let out = bcx.ins().stack_addr(types::I64, slot, 0);
    let mut sig = module.make_signature();
    for _ in 0..4 {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let id = module
        .declare_function(helper, Linkage::Import, &sig)
        .ok()?;
    let f = module.declare_func_in_func(id, bcx.func);
    let want_v = bcx.ins().iconst(types::I64, want);
    let call = bcx.ins().call(f, &[t, key, want_v, out]);
    let ok = bcx.inst_results(call)[0];
    let slow_bits = bcx.ins().stack_load(types::I64, types::I64, slot, 0);
    bcx.ins()
        .brif(ok, merge_blk, &[BlockArg::Value(slow_bits)], deopt_blk, &[]);

    bcx.switch_to_block(deopt_blk);
    bcx.seal_block(deopt_blk);
    let park_sig = module.make_signature();
    let park_id = module
        .declare_function("luna_jit_park_deopt", Linkage::Import, &park_sig)
        .ok()?;
    let park = module.declare_func_in_func(park_id, bcx.func);
    bcx.ins().call(park, &[]);
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.switch_to_block(merge_blk);
    bcx.seal_block(merge_blk);
    Some(bcx.block_params(merge_blk)[0])
}

/// backend-agnostic body of the int-chunk
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
// cranelift types in the signature: internal to luna crates, not covered by semver
#[doc(hidden)]
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
    // luna's `local function f(...) end` idiom binds upvalue 0
    // (Lua 5.5/5.4/5.3/5.2) or upvalue 1 (Lua 5.1 — slot 0 is the
    // `_ENV` placeholder) to the closure itself. Upvalue
    // tracking is general: the scanner watches GetUpval(b) and pins the
    // self-upval index from the first occurrence; subsequent
    // GetUpval(b') with b' != self-upval-idx bails. Upvals count is
    // bounded only to avoid pathological cases.
    let allows_self_recursion = !proto.upvals.is_empty() && proto.upvals.len() <= 4;
    let mut self_upval_idx: Option<u32> = None;
    // First pass: verify every op is supported AND scan for basic-block
    // boundaries. A BB starts at PC 0, at every jump target, and at the
    // instruction immediately after a terminator (Jmp, Return, or a
    // paired Lt|Le|Eq+Jmp).
    // the passes below read register operands only
    let first_scratch = (proto.max_stack as usize).max(num_params);
    let code = const_operands::split_const_operands(&proto, first_scratch)?;
    let code = &code[..];
    let n = code.len();
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
    // two more registers: the constant operands' scratch (`split_const_operands`)
    let max_stack = (proto.max_stack as usize).max(num_params) + 2;
    let mut self_upval: Vec<bool> = vec![false; max_stack];
    // per-PC role for `Op::GetUpval`. SelfMarker (true at
    // the bool position is misleading — see the enum-like split below)
    // is the call-target shortcut; ValueRead enables fetching the
    // upvalue value at runtime via `luna_jit_upval_get` so chunks like
    // `function () return k * k end` can JIT. `is_upval_value_read[pc]`
    // is true iff the role is ValueRead. Pre-pass below decides via
    // an 8-op lookahead from each `GetUpval`.
    let is_upval_value_read: Vec<bool> = determine_getupval_roles(code);
    // PC of every Op::Call that resolves to the self-recursion edge.
    // Emit-side consumes this to lower as a cranelift `call fn_id`.
    let mut self_call_pcs: Vec<bool> = vec![false; n];
    // track each register's last-written `LoadI` immediate (or
    // None when it was overwritten by anything else). `ForPrep` reads
    // `step_const[A+2]` to check that the step is a compile-time
    // constant ≠ 0 — non-immediate steps bail to the interpreter.
    let mut step_const: Vec<Option<i64>> = vec![None; max_stack];
    // every JIT'd ForPrep/ForLoop pair, in source order. Each
    // tuple is `(prep_pc, loop_pc, step_imm)`. Emit consumes this to
    // lay out the counted-loop blocks.
    let mut for_loops: Vec<(usize, usize, i64)> = Vec::new();

    // `defines_table[reg]` tracks whether a `NewTable` or
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
    // function params are guaranteed defined by the
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

    // per-PC presize hint for `Op::NewTable`. When a
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

    // pre-scan: detect `math.<fn>(arg)` 4-op folds. The
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
            if let Some(fold) = try_match_math_fold(&proto, code, try_pc, float_only) {
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
    // The folds are checked once, at entry; a table store in the body
    // could reassign a math field after that.
    if !math_folds.is_empty()
        && code.iter().any(|i| {
            matches!(
                i.op(),
                Op::SetTable | Op::SetI | Op::SetField | Op::SetTabUp
            )
        })
    {
        return None;
    }

    let mut pc = 0;
    while pc < n {
        let ins = code[pc];
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
                // Float constants pass. Int constants also
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
                    // a `LoadK Int(v)` also pins the register
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
                // `R[A..=A+B] = nil`. The whitelist accepts
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
                // propagate table-defined-ness through
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
                // Bail out — only the call-target case is handled.
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
                // ValueRead role: `R[A]` is consumed as
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
                // SelfMarker — self-recursion call target.
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
                // variadic Call (C=0) paired with a
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
                    && matches!(code[pc + 1].op(), Op::SetList)
                    && code[pc + 1].b() == 0;
                let nresults = if next_is_variadic_setlist {
                    1
                } else {
                    c.checked_sub(1)?
                };
                if nargs > MAX_JIT_ARITY as u32 || nresults != 1 {
                    return None;
                }
                if folded_math[pc] {
                    // math libcall fold. Emit-side folds the
                    // 4-op window into one cranelift libm call; here
                    // we just clear the per-register trackers.
                } else if self_upval.get(a).copied().unwrap_or(false)
                    && nargs as usize == num_params
                {
                    // self-recursive call, lowered as a direct
                    // call of the compiled body, whose signature takes
                    // exactly the function's parameters. The upvalue may
                    // hold another function (the entry check catches that
                    // at run time), so the call site's count can differ;
                    // such a call is not lowered.
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
                // accepted only as part of a recognized
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
                // a shape the lowerer handles. Bail.
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
                // a jump to itself (`while true do end`, `::l:: goto l`)
                // would spin in native code, where the interpreter's
                // instruction budget and hooks never run
                if tgt >= n || tgt == pc {
                    return None;
                }
                bb_starts[tgt] = true;
                if pc + 1 < n {
                    bb_starts[pc + 1] = true;
                }
            }
            Op::Lt | Op::Le | Op::Eq => {
                // Reading a tagged register here is a generic-upvalue
                // comparison (e.g. `if n_upval < 3 then …`) which the
                // lowerer doesn't model.
                if self_upval.get(ins.a() as usize).copied().unwrap_or(false)
                    || self_upval.get(ins.b() as usize).copied().unwrap_or(false)
                {
                    return None;
                }
                // A comparison op is always paired with a following Jmp
                // (PUC's `cond_skip` invariant). luna's compiler never
                // emits one without the other; if we see a lone Lt/Le/Eq
                // the proto is malformed for our purposes — bail out.
                let &jmp = code.get(pc + 1)?;
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
                // both forms admitted. The dialect-
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
                let loop_ins = code[loop_pc];
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
                // empty-table form. luna's frontend emits
                // NewTable a=A b=0 c=0 for `{}`.
                // Also accept `b > 0` (array presize for
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
                // register-keyed set. The proper safety
                // gate (R[A] must be a definitively-defined table at
                // this PC) lives in the BB-level dataflow check
                // below; the linear `defines_table` walk would
                // wrongly accept a false-branch-only NewTable.
            }
            Op::SetList => {
                // fixed-count array literal initializer
                // (B > 0). Variadic form (B == 0, C ==
                // 0) accepted when paired with the immediately
                // preceding `Op::Call C=0`; the JIT'd self-recursive
                // callee returns exactly 1 value, so the static
                // count is `A_call - A_list`.
                let b = ins.b();
                // the emit stores from index 1: no offset, and no
                // `ExtraArg` offset either
                if ins.c() != 0 || ins.k() {
                    return None;
                }
                if b == 0 {
                    if pc == 0 {
                        return None;
                    }
                    let prev = code[pc - 1];
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
                // `R[A] = R[B][imm(C)]`. BB-level dataflow
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
                // `R[A] = R[B][R[C]]`. BB-level dataflow
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
                // `R[A] = #R[B]`. BB-level dataflow
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

    // BB-level dataflow for "is this register a
    // table at the use site". A blanket
    // `has_new_table && has_conditional → bail` safety
    // net would be sound but would reject the make-style
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
            let ins = code[p];
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
                    let jmp = code[p + 1];
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
            let ins = code[p];
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
                    // LoadNil writes Nil to R[A..=A+B];
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

    // "must-defined" dataflow uses intersection at
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
        let ins = code[p];
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
                let prev = code[q];
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

    // find every NewTable that opens a
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

        let nt = code[nt_pc];
        let init = code[init_pc];
        let limit = code[limit_pc];
        let step = code[step_pc];
        let fp = code[prep_pc];

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

    // every math fold's internal PCs (+1, +2, +3) must
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

    // Correctness gate: every JIT-recognised self-recursive call
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
            let ins = code[pc];
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
                    let jmp = code[pc + 1];
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

    // per-register type inference. Each Lua register holds either
    // an Int (i64) or a Float (f64). A register that's pinned to both
    // shapes within the same Proto bails the lowerer. The sweep is
    // forward-only with a fixpoint loop because a self-recursive Call
    // result kind depends on the Proto's own return kind (carried via
    // `ret_kind`); successive passes propagate the resolved kind.
    let mut reg_kinds: Vec<RegKind> = vec![RegKind::Unset; max_stack];
    let mut ret_kind: RegKind = RegKind::Unset;
    // `latest_writer_kind[reg]` records the kind written to `reg`
    // by the most recent writer op in linear PC order during this
    // sweep pass. With the current per-proto `RegKind` model
    // (strict Int/Table conflict), this tracker is a no-op: every
    // op that writes a Variable's kind also passes through the
    // global unify, so latest_writer_kind never disagrees with
    // reg_kinds. The scaffold is wired in so a relaxed
    // `unify` (e.g. Int + Table → joint) lets the Return1 ret
    // kind can be picked from the latest writer rather than the
    // joint kind. See `make_proto_5_5_round_trip` (currently still
    // bails) for the motivating shape.
    let mut latest_writer_kind: Vec<RegKind>;
    // `maybe_table[reg]` is set when the register
    // could hold a Table pointer at runtime even though
    // `reg_kinds[reg]` says Int. The classic case is
    // `Op::GetI R[A] = R[B][c]`: the helper returns the raw
    // payload bits regardless of the stored Value's tag, so
    // when the slot held a Table at runtime R[A] is a Gc<Table>
    // pun. Downstream arith / Lt-Le / ForPrep use this tag to
    // bail conservatively (interp would have raised; the JIT'd
    // `iadd` / `icmp` would silently compute garbage).
    let mut maybe_table: Vec<bool>;
    // parallel to `maybe_table`: this register's most
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
        // (filed 2026-06-22 by luna-core/tests/it/e2e_programs.rs::err_arith_on_nil).
        is_nil_writer = vec![false; max_stack];
        for r in num_params..max_stack {
            is_nil_writer[r] = true;
        }
        let mut pc = 0;
        while pc < n {
            let ins = code[pc];
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
                    // `R[A..=A+B] = nil`. Leave `reg_kinds`
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
                    // fold-internal Move (slot +2 of a math
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
                    // Table operand makes Lua's interp
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
                    // `nil + x` / `x + nil` raises in interp
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
                    // Lt/Le errors on a Table; Eq is
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
                    // `nil < x` / `nil <= x` raise; `nil == x`
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
                    // no kind constraint for the SelfMarker role.
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
                    // ValueRead role: pin R[A] to Float so
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
                        // pin R[A] (= Call.A = the fold's
                        // result slot) to the fold's result kind.
                        let k = math_folds
                            .iter()
                            .find(|f| f.start_pc + 3 == pc)
                            .map_or(RegKind::Float, MathFold::result_kind);
                        if !RegKind::unify(&mut reg_kinds[ins.a() as usize], k) {
                            return None;
                        }
                        latest_writer_kind[ins.a() as usize] = k;
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
                    // folded GetTabUp / GetField don't ever
                    // observe their stored values (the next fold op
                    // overwrites R[A]). The Call PC pins R[A] to
                    // Float on its own; nothing to do here.
                    if !folded_math[pc] {
                        return None;
                    }
                }
                Op::Return1 => {
                    // Return1 on a LoadNil-written register
                    // would wrap `Int(0)` instead of `Nil` (the helper
                    // ABI is i64 bits; the dispatcher uses ret_kind to
                    // decide Int vs Float, not Nil). Bail to interp so
                    // a `function () return nil end` returns Nil, not
                    // Int(0).
                    if is_nil_writer[ins.a() as usize] {
                        return None;
                    }
                    // pick from the most recent writer
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
                    // guard on Unset: a 5.1/5.2
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
                    // Int loop, or Float loop (5.1 /
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
                    // with the relaxed Int+Table `unify`
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
                    // Likewise a nil-written init / limit / step (`for i =
                    // 1, nil`, or a declared-uninitialized local): the
                    // interpreter raises the 'for' error, the JIT would
                    // loop over the Variable's zero payload.
                    for off in [0usize, 1, 2, 3] {
                        if matches!(latest_writer_kind[a + off], RegKind::Table)
                            || maybe_table[a + off]
                            || (off < 3 && is_nil_writer[a + off])
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
                    // R[A] = fresh empty table.
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
                    // `R[A][1..=B] = R[A+1..A+B]`. R[A]
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
                    // R[A] (table) Table. Key/value pair must
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
                    // `t[nil] = x` raises in interp (
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
                    // R[A] = R[B][imm(C)]. R[B] must be Table;
                    // R[A] is Int (matches the static Int-only store
                    // expectation of `luna_jit_table_get_int`).
                    // the helper returns raw payload
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
                    // R[A] = R[B][R[C]]. R[B] is Table.
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
                    // Nil key would call `Table::get(Nil)`
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
                    // Float default. See the 5.3 test
                    // `tests/it/jit_dialect_audit.rs::audit_gettable_computed_key`.
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
                    // R[A] = #R[B]. R[B] Table; R[A] holds the
                    // Int length helper return.
                    let a = ins.a() as usize;
                    let b = ins.b() as usize;
                    if !RegKind::unify(&mut reg_kinds[b], RegKind::Table) {
                        return None;
                    }
                    // Len's i64 helper return goes through
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
    // Table-typed params go through the dispatcher's
    // `Value::Table` marshalling path (`arg_table_mask`); they
    // pass the raw `Gc<Table>` ptr as the i64 ABI slot.
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

    // per-BB RegKind dataflow.
    //
    // `bb_entry_kinds[bb][r]` is the active kind (latest-writer kind on
    // every path reaching this BB) for register `r` at the BB's entry
    // PC. emit-time `current_kinds` resets to this on every BB switch
    // so an alternate-path writer's kind doesn't leak into the
    // current path. Readers gate behind this so Float-vs-Table
    // register reuse across BBs can unify globally (the 5.1/5.2
    // binary_trees + table_alloc shapes).
    //
    // `Op::SetList` reads regs it just wrote inside the same BB
    // (writers always immediately precede the SetList), so the reset
    // never changes SetList's view.
    //
    // Lattice:
    //   TOP = `RegKind::Unset` (initial non-entry BB entry; encodes
    //         "no info yet" during fixpoint and "fall back to declared
    //         `reg_kinds`" at emit time).
    //   `Int` / `Float` / `Table` = definite kinds.
    //   meet(X, X) = X; meet(X, Unset) = X; meet(X, Y) for X ≠ Y =
    //   Unset (join conflict — emit-side readers fall back).
    //
    // Mirrors the `defines_table` dataflow shape: forward, fixed
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
            let ins = code[p];
            // Math fold: the underlying GetField / Move / Call inside
            // a fold are skipped at emit (`pc += 3` after the
            // GetTabUp), so their would-be writes don't happen. Only
            // the GetTabUp at `start_pc` actually writes
            // `fold.dst_reg = Float`.
            if folded_math[p] {
                if let Some(fold) = math_folds.iter().find(|f| f.start_pc == p)
                    && let Some(slot) = state.get_mut(fold.dst_reg as usize)
                {
                    *slot = fold.result_kind();
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
                    if !matches!(ret_kind, RegKind::Unset)
                        && let Some(slot) = state.get_mut(ins.a() as usize)
                    {
                        *slot = ret_kind;
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
                    // emit writes iconst(0) into each
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
    // two more registers: the constant operands' scratch (`split_const_operands`)
    let max_stack = (proto.max_stack as usize).max(num_params) + 2;
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
                bcx.ins().bitcast(types::F64, MemFlagsData::new(), raw)
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

    // emit-side per-PC kind tracker. Initialized from
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
    // parallel to `current_kinds`: tracks "the value
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
            // reset emit-side `current_kinds` to
            // the per-BB dataflow result so an alternate-path
            // writer's kind doesn't leak forward. The linear writer
            // updates below continue to refine `current_kinds` as
            // emit progresses through the new BB.
            let new_bb_idx = pc_to_bb[pc];
            current_kinds = bb_entry_kinds[new_bb_idx].clone();
            // Nil writes don't cross BB joins in the
            // patterns we lower; reset rather than fold them into
            // a separate per-BB dataflow.
            for slot in current_is_nil.iter_mut() {
                *slot = false;
            }
        }
        let _ = current_block; // tracked only for parity assertions in tests.
        let ins = code[pc];
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
                // `R[A..=A+B] = nil`. Lower to a sequence of
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
                // a register a table was stored in first keeps the Table
                // kind when a later arithmetic result lands there; its
                // result tag would be wrong, so leave the function to the
                // interpreter
                if k == RegKind::Table {
                    return None;
                }
                // 5.1/5.2 integers stand for doubles, whose sums round and
                // whose products can be -0: the interpreter's to compute
                if float_only && k != RegKind::Float {
                    return None;
                }
                // A float result converts an integer operand first
                // (`a / b` of two integers, or `i + 0.5`).
                let (lhs, rhs) = if k == RegKind::Float {
                    let to_float = |bcx: &mut FunctionBuilder<'_>, v: Value| {
                        if bcx.func.dfg.value_type(v) == types::I64 {
                            bcx.ins().fcvt_from_sint(types::F64, v)
                        } else {
                            v
                        }
                    };
                    (to_float(&mut bcx, lhs), to_float(&mut bcx, rhs))
                } else {
                    (lhs, rhs)
                };
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
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
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
                // emit-side fold consumer. PCs +1..+3 are
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

                let arg_kind = a_kind(&reg_kinds, fold.arg_reg);
                let arg_var = bcx.use_var(regs[fold.arg_reg as usize]);
                let result = if fold.int_result {
                    match arg_kind {
                        RegKind::Float => {
                            let r = if fold.fn_name == "floor" {
                                bcx.ins().floor(arg_var)
                            } else {
                                bcx.ins().ceil(arg_var)
                            };
                            // An integer when it fits (NaN and the
                            // infinities do not); otherwise the result
                            // is a float, a kind this register cannot
                            // hold, and the interpreter reruns the call.
                            // Folds only compile in chunks without
                            // table stores, so nothing has happened yet
                            // that a rerun would repeat.
                            let lo = bcx.ins().f64const(-9_223_372_036_854_775_808.0);
                            let hi = bcx.ins().f64const(9_223_372_036_854_775_808.0);
                            let ge_lo = bcx.ins().fcmp(FloatCC::GreaterThanOrEqual, r, lo);
                            let lt_hi = bcx.ins().fcmp(FloatCC::LessThan, r, hi);
                            let fits = bcx.ins().band(ge_lo, lt_hi);
                            let ok_blk = bcx.create_block();
                            let bail_blk = bcx.create_block();
                            bcx.ins().brif(fits, ok_blk, &[], bail_blk, &[]);
                            bcx.switch_to_block(bail_blk);
                            bcx.seal_block(bail_blk);
                            let park_id = module
                                .declare_function(
                                    "luna_jit_park_deopt",
                                    Linkage::Import,
                                    &module.make_signature(),
                                )
                                .ok()?;
                            let park_ref = module.declare_func_in_func(park_id, bcx.func);
                            bcx.ins().call(park_ref, &[]);
                            let zero = bcx.ins().iconst(types::I64, 0);
                            bcx.ins().return_(&[zero]);
                            bcx.switch_to_block(ok_blk);
                            bcx.seal_block(ok_blk);
                            bcx.ins().fcvt_to_sint(types::I64, r)
                        }
                        // An integer is its own floor and ceiling.
                        RegKind::Int | RegKind::Unset => arg_var,
                        // `math.floor(t)` raises in the interpreter
                        RegKind::Table => return None,
                    }
                } else {
                    let arg_f64 = match arg_kind {
                        RegKind::Float => arg_var,
                        RegKind::Int | RegKind::Unset => {
                            bcx.ins().fcvt_from_sint(types::F64, arg_var)
                        }
                        // `math.sin(t)` raises in the interpreter
                        RegKind::Table => return None,
                    };
                    // 5.3+ `atan(y)` is `atan2(y, 1)` (lmathlib.c), which
                    // libm rounds differently from `atan(y)`.
                    let atan2 = fold.fn_name == "atan" && !float_only;
                    let mut libm_sig = module.make_signature();
                    libm_sig.params.push(AbiParam::new(types::F64));
                    if atan2 {
                        libm_sig.params.push(AbiParam::new(types::F64));
                    }
                    libm_sig.returns.push(AbiParam::new(types::F64));
                    let name = if atan2 { "atan2" } else { fold.fn_name };
                    let libm_id = module
                        .declare_function(name, Linkage::Import, &libm_sig)
                        .ok()?;
                    let libm_ref = module.declare_func_in_func(libm_id, bcx.func);
                    let call_inst = if atan2 {
                        let one = bcx.ins().f64const(1.0);
                        bcx.ins().call(libm_ref, &[arg_f64, one])
                    } else {
                        bcx.ins().call(libm_ref, &[arg_f64])
                    };
                    bcx.inst_results(call_inst)[0]
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, fold.dst_reg as usize, result);
                current_kinds[fold.dst_reg as usize] = fold.result_kind();
                current_is_nil[fold.dst_reg as usize] = false;

                pc += 3; // skip GetField + Move + Call; outer `pc += 1` lands past the Call.
            }
            Op::GetField if folded_math[pc] => {
                unreachable!("GetTabUp emit advances pc past the rest of the fold");
            }
            Op::GetUpval => {
                let a = ins.a() as usize;
                if is_upval_value_read[pc] {
                    // ValueRead: fetch the upvalue at
                    // runtime via `luna_jit_upval_get_float`, which deopts
                    // on anything but a float. The dispatcher
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
                        .declare_function("luna_jit_upval_get_float", Linkage::Import, &sig)
                        .ok()?;
                    let r = module.declare_func_in_func(id, bcx.func);
                    let call_inst = bcx.ins().call(r, &[idx_arg]);
                    let v = bcx.inst_results(call_inst)[0];
                    aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                    current_kinds[a] = reg_kinds[a];
                    current_is_nil[a] = false;
                } else {
                    // SelfMarker placeholder. The matching
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
                        bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
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
                    bcx.ins()
                        .bitcast(types::F64, MemFlagsData::new(), result_i64)
                } else {
                    result_i64
                };
                aligned_def(&mut bcx, &regs, &reg_kinds, a, result);
                // self-recursive call returns ret_kind.
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
                        // pre-5.3 Int form. R[A] = init - step
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
                        // 5.4+ Int count form.
                        let init = bcx.use_var(regs[a]);
                        let limit = bcx.use_var(regs[a + 1]);

                        let empty = if step_imm > 0 {
                            bcx.ins().icmp(IntCC::SignedGreaterThan, init, limit)
                        } else {
                            bcx.ins().icmp(IntCC::SignedLessThan, init, limit)
                        };

                        // count = (limit - init) / step (positive-step)
                        //       = (init - limit) / -step (negative-step)
                        // Both are unsigned (PUC `lua_Unsigned`): the span
                        // of a loop over most of the integer range does not
                        // fit an i64.
                        let span = if step_imm > 0 {
                            bcx.ins().isub(limit, init)
                        } else {
                            bcx.ins().isub(init, limit)
                        };
                        // `math.mininteger` as a step: its magnitude is
                        // 2^63, which only the unsigned division sees right
                        let abs_step = bcx.ins().iconst(types::I64, step_imm.unsigned_abs() as i64);
                        let count = bcx.ins().udiv(span, abs_step);

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
                        // pre-5.3 Float form. R[A] = init - step,
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
                        // 5.4+ Float form. Mirrors interp's
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
                    // Float ForLoop. Same shape for pre53 and
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
                    // pre-5.3 Int form. R[A] += step; check vs
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
                    // 5.4+ Int count form.
                    let count = bcx.use_var(regs[a + 1]);
                    let zero_i = bcx.ins().iconst(types::I64, 0);
                    // unsigned count (see ForPrep)
                    let cont = bcx.ins().icmp(IntCC::NotEqual, count, zero_i);

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
                let jmp = code[pc + 1];
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
                // `R[A] = {}` lowers to a call into the
                // `luna_jit_new_table` Rust helper. The helper reads
                // the active Vm pointer from the thread-local set by
                // `enter_jit`. Result is the `Gc<Table>` pointer
                // pun'd to I64, written into R[A].
                //
                // when the scan recorded a presize hint
                // (the NewTable opens a counted `for i = 1, N`
                // window), reach for the `_sized` variant with N
                // as an i64 const arg. Skips the O(log N) rehash
                // chain that would otherwise dominate the loop.
                //
                // also honour `NewTable.B` as a presize
                // hint: luna's frontend emits `NewTable A B=N` for
                // `{a, b, c, ...}` literals (the SetList that
                // follows fills exactly N entries). Either source —
                // the scanned window or NewTable.B — feeds the sized
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
                // `R[A][R[B]] = R[C]`. Pick the Int/Int vs
                // Float/Float helper at emit time based on R[B]'s
                // resolved kind (the scan pinned R[B] and R[C] to
                // the same kind).
                //
                // for the Int/Int variant, emit an inline
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
                // when `R[A]` is Float-declared
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
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
                } else {
                    t_raw
                };
                let key = bcx.use_var(regs[b]);
                let val = bcx.use_var(regs[c]);
                let is_float = matches!(a_kind(&reg_kinds, b as u32), RegKind::Float);

                if !is_float {
                    // Inline aset fast path (Int key + Int val).
                    // load `asize` (u64) once for both the
                    // in-range check and the `atags_ptr = avals_ptr +
                    // asize * 8` computation. Avals occupy `slab` from
                    // offset 0; atags trail at byte offset `asize * 8`.
                    let asize = bcx.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
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
                        MemFlagsData::trusted(),
                        t,
                        TABLE_ARRAY_PTR_OFFSET as i32,
                    );
                    // atags_ptr = avals_ptr + asize * 8
                    let three = bcx.ins().iconst(types::I64, 3);
                    let avals_bytes = bcx.ins().ishl(asize, three);
                    let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
                    let tag_dst = bcx.ins().iadd(atags_ptr, key_minus_1);
                    let old_tag = bcx
                        .ins()
                        .uload8(types::I64, MemFlagsData::trusted(), tag_dst, 0);
                    let tag_byte = bcx.ins().iconst(types::I8, RAW_TAG_INT);
                    bcx.ins()
                        .store(MemFlagsData::trusted(), tag_byte, tag_dst, 0);
                    let val_off = bcx.ins().ishl(key_minus_1, three); // *8
                    let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                    bcx.ins().store(MemFlagsData::trusted(), val, val_dst, 0);
                    emit_array_fill_count(&mut bcx, t, key_minus_1, old_tag, merge_blk);

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
                    let key_i = bcx.ins().bitcast(types::I64, MemFlagsData::new(), key);
                    let val_i = bcx.ins().bitcast(types::I64, MemFlagsData::new(), val);
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
                // `R[A][1..=B] = R[A+1..A+B]`. Each store goes inline
                // through the atags/avals layout the SetTable fast path
                // uses when the array part holds all B slots; the table
                // is normally the preceding `NewTable` presized to B, but
                // that is not proven here, so a smaller array part takes
                // the helper path, which grows the table. Each element's
                // tag is picked at emit time from `RegKind[A+i]`:
                //   Int     → raw::INT     (i64 verbatim)
                //   Float   → raw::FLOAT   (bitcast f64 → i64)
                //   Table   → raw::TABLE   (i64 ptr verbatim)
                //
                // `B == 0` variadic form: the matching
                // preceding `Op::Call C=0` returns exactly 1 value
                // (the self-recursive callee's `returns_one == true`
                // guarantee), so the static count is
                // `A_call - A_list`. Source regs are still
                // `R[A+1..A+count]`.
                let a = ins.a() as usize;
                let b_field = ins.b();
                let b = if b_field == 0 {
                    let prev = code[pc - 1];
                    (prev.a() as usize).saturating_sub(a)
                } else {
                    b_field as usize
                };
                let t_raw = bcx.use_var(regs[a]);
                // Float-declared Table operand
                // bitcast to I64; see SetTable for the rationale.
                let t = if matches!(
                    reg_kinds.get(a).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
                } else {
                    t_raw
                };
                let mut elems = Vec::with_capacity(b);
                for i in 0..b {
                    let src = a + 1 + i;
                    let v = bcx.use_var(regs[src]);
                    // per-PC kind from `current_kinds`,
                    // not the global `reg_kinds`. R[A+i] may legitimately
                    // hold an Int at one SetList PC and a Table at
                    // another (the binary_trees `make` pattern).
                    let kind = current_kinds.get(src).copied().unwrap_or(RegKind::Int);
                    // collapse to I64 first
                    // (lossless when declared F64), then pick the
                    // tag. Handles all (declared × active) ∈ {F64,
                    // I64} × {Int, Float, Table} correctly: a
                    // Float-declared slot whose active kind here is
                    // Int or Table still stores its 8-byte payload
                    // verbatim under the right tag.
                    let is_nil_src = current_is_nil.get(src).copied().unwrap_or(false);
                    let (tag, bits) = if is_nil_src {
                        // slot was last written by LoadNil
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
                            bcx.ins().bitcast(types::I64, MemFlagsData::new(), v)
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
                    elems.push((tag, bits));
                }
                let asize = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let b_v = bcx.ins().iconst(types::I64, b as i64);
                let fits = bcx
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, asize, b_v);
                // the inline stores fill an all-nil array part (a fresh
                // constructor table) with non-nil values, so afterwards
                // `acount` and `aprefix` are both `b`; anything else takes
                // the helper path, which keeps them itself
                let fits = if elems.iter().all(|&(tag, _)| tag != RAW_TAG_NIL) {
                    let acount =
                        bcx.ins()
                            .load(types::I32, MemFlagsData::trusted(), t, TABLE_ACOUNT_OFFSET);
                    let empty = bcx.ins().icmp_imm_u(IntCC::Equal, acount, 0);
                    bcx.ins().band(fits, empty)
                } else {
                    bcx.ins().iconst(types::I8, 0)
                };
                let fast_blk = bcx.create_block();
                let slow_blk = bcx.create_block();
                let merge_blk = bcx.create_block();
                bcx.ins().brif(fits, fast_blk, &[], slow_blk, &[]);

                bcx.switch_to_block(fast_blk);
                bcx.seal_block(fast_blk);
                // `atags_ptr = avals_ptr + asize * 8`, once for the
                // whole literal
                let avals_ptr = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ARRAY_PTR_OFFSET as i32,
                );
                let three_imm = bcx.ins().iconst(types::I64, 3);
                let avals_bytes = bcx.ins().ishl(asize, three_imm);
                let atags_ptr = bcx.ins().iadd(avals_ptr, avals_bytes);
                for (i, &(tag, bits)) in elems.iter().enumerate() {
                    let idx_const = bcx.ins().iconst(types::I64, i as i64);
                    let tag_dst = bcx.ins().iadd(atags_ptr, idx_const);
                    let tag_byte = bcx.ins().iconst(types::I8, tag);
                    bcx.ins()
                        .store(MemFlagsData::trusted(), tag_byte, tag_dst, 0);
                    let val_off = bcx.ins().iconst(types::I64, (i as i64) * 8);
                    let val_dst = bcx.ins().iadd(avals_ptr, val_off);
                    bcx.ins().store(MemFlagsData::trusted(), bits, val_dst, 0);
                }
                let filled = bcx.ins().iconst(types::I32, b as i64);
                bcx.ins()
                    .store(MemFlagsData::trusted(), filled, t, TABLE_ACOUNT_OFFSET);
                bcx.ins()
                    .store(MemFlagsData::trusted(), filled, t, TABLE_APREFIX_OFFSET);
                bcx.ins().jump(merge_blk, &[]);

                bcx.switch_to_block(slow_blk);
                bcx.seal_block(slow_blk);
                let mut sig = module.make_signature();
                for _ in 0..4 {
                    sig.params.push(AbiParam::new(types::I64));
                }
                let id = module
                    .declare_function("luna_jit_table_set_raw", Linkage::Import, &sig)
                    .ok()?;
                let r = module.declare_func_in_func(id, bcx.func);
                for (i, &(tag, bits)) in elems.iter().enumerate() {
                    let key = bcx.ins().iconst(types::I64, i as i64 + 1);
                    let tag_v = bcx.ins().iconst(types::I64, tag);
                    let _ = bcx.ins().call(r, &[t, key, bits, tag_v]);
                }
                bcx.ins().jump(merge_blk, &[]);

                bcx.switch_to_block(merge_blk);
                bcx.seal_block(merge_blk);
            }
            Op::GetI => {
                // `R[A] = R[B][imm(C)]`. Inline
                // aget fast path: when the immediate `C` key fits the
                // array part AND the table has no metatable, load the
                // raw 8-byte payload from `array_ptr[key-1] * 8`
                // directly. Mirrors the inline aset shape:
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
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
                } else {
                    t_raw
                };
                let key_imm = ins.c() as i64;

                let asize = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let key_minus_1 = bcx.ins().iconst(types::I64, key_imm - 1);
                let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
                let metatable = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_METATABLE_OFFSET as i32,
                );
                let zero_i64 = bcx.ins().iconst(types::I64, 0);
                let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
                let fast_ok = bcx.ins().band(in_range, no_meta);

                let key = bcx.ins().iconst(types::I64, key_imm);
                let want = want_tag(reg_kinds.get(a).copied().unwrap_or(RegKind::Int));
                let v = emit_checked_get(
                    &mut bcx,
                    module,
                    t,
                    fast_ok,
                    key_minus_1,
                    ("luna_jit_table_get_int_checked", key),
                    want,
                )?;
                aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                current_kinds[a] = reg_kinds[a];
                current_is_nil[a] = false;
            }
            Op::GetTable => {
                // `R[A] = R[B][R[C]]`. Same fast
                // path shape as the GetI inline aget, but the
                // key sits in a register rather than as an immediate.
                // Float keys (5.1/5.2 `t[1.0]`) get an exactness check
                // (in the i64 range, and fcvt + fcvt back == original)
                // before the bounds + metatable guards; NaN, infinite,
                // out-of-range and fractional keys fall through to the
                // helper which walks the hash part. Int keys (5.3+) skip
                // the fcvt round-trip.
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let c = ins.c() as usize;
                let t_raw = bcx.use_var(regs[b]);
                let t = if matches!(
                    reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
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
                    // the saturating form: the trapping one kills the
                    // process on a NaN or out-of-range key
                    let key_int = bcx.ins().fcvt_to_sint_sat(types::I64, key_raw);
                    let key_back = bcx.ins().fcvt_from_sint(types::F64, key_int);
                    let round_trips = bcx.ins().fcmp(FloatCC::Equal, key_raw, key_back);
                    // 2^63 saturates to i64::MAX, which converts back to
                    // 2^63: only the range check tells it apart
                    let fits = trace::emit_f64_fits_i64(&mut bcx, key_raw);
                    let exact = bcx.ins().band(round_trips, fits);
                    (key_int, exact)
                } else {
                    // Int key — always "exact" by construction.
                    let always = bcx.ins().iconst(types::I8, 1);
                    (key_raw, always)
                };

                let asize = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_ASIZE_OFFSET as i32,
                );
                let one = bcx.ins().iconst(types::I64, 1);
                let key_minus_1 = bcx.ins().isub(key_i64, one);
                let in_range = bcx.ins().icmp(IntCC::UnsignedLessThan, key_minus_1, asize);
                let metatable = bcx.ins().load(
                    types::I64,
                    MemFlagsData::trusted(),
                    t,
                    TABLE_METATABLE_OFFSET as i32,
                );
                let zero_i64 = bcx.ins().iconst(types::I64, 0);
                let no_meta = bcx.ins().icmp(IntCC::Equal, metatable, zero_i64);
                let bounds_ok = bcx.ins().band(in_range, no_meta);
                let fast_ok = bcx.ins().band(bounds_ok, key_ok);

                let slow = if is_float_key {
                    let key_bits = bcx.ins().bitcast(types::I64, MemFlagsData::new(), key_raw);
                    ("luna_jit_table_get_float_checked", key_bits)
                } else {
                    ("luna_jit_table_get_int_checked", key_raw)
                };
                let want = want_tag(reg_kinds.get(a).copied().unwrap_or(RegKind::Int));
                let v = emit_checked_get(&mut bcx, module, t, fast_ok, key_minus_1, slow, want)?;
                aligned_def(&mut bcx, &regs, &reg_kinds, a, v);
                current_kinds[a] = reg_kinds[a];
                current_is_nil[a] = false;
            }
            Op::Len => {
                // `R[A] = #R[B]`.
                let a = ins.a() as usize;
                let b = ins.b() as usize;
                let t_raw = bcx.use_var(regs[b]);
                // Float-declared table operand
                // bitcast to I64; see SetTable.
                let t = if matches!(
                    reg_kinds.get(b).copied().unwrap_or(RegKind::Int),
                    RegKind::Float
                ) {
                    bcx.ins().bitcast(types::I64, MemFlagsData::new(), t_raw)
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
    bcx.finalize(module.target_config());

    module.define_function(fn_id, &mut ctx).ok()?;
    module.clear_context(&mut ctx);

    // The body's self-recursive calls go straight to its own code, which
    // is the Lua call only while the upvalue they load holds the running
    // closure, and its math folds replace `math.<fn>(...)` by inline code,
    // which is the Lua call only while the field holds the library
    // function. The compiled code is shared by every closure of the proto
    // (and by protos with the same code), so both are checked on each
    // entry from the interpreter. Recursive calls enter the body directly:
    // nothing the body runs can reassign the upvalue or, with no table
    // stores (checked above), a field.
    let mut math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)> = Vec::new();
    for fold in &math_folds {
        if !math_fns.iter().any(|&(_, n)| n.ptr_eq(fold.name_key)) {
            math_fns.push((fold.math_key, fold.name_key));
        }
    }
    let checks = EntryChecks {
        self_upval: self_upval_idx.filter(|_| any_self_call),
        math_fns,
    };
    let entry_id = if checks.self_upval.is_some() || !checks.math_fns.is_empty() {
        define_checked_entry(module, &mut ctx, fn_id, &checks, num_params)?
    } else {
        fn_id
    };

    // diag of the lowered chunk's shape lives in the runtime
    // wrapper [`try_compile_int_chunk`]. The generic
    // body only emits the function; finalize is the caller's job.
    let _ = ret_kind; // tracked for diag in the JIT wrapper; backend-agnostic here.

    Some((
        entry_id,
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

/// What a chunk's entry verifies before running the body.
struct EntryChecks {
    /// Upvalue the self-recursive calls go through.
    self_upval: Option<u32>,
    /// `("math", name)` key pairs of the folded `math.<name>` calls.
    math_fns: Vec<(Gc<LuaStr>, Gc<LuaStr>)>,
}

/// Defines the entry that runs `checks` before calling the chunk body
/// `body_id`: when one fails it returns at once with a deopt parked, and
/// the dispatcher runs the call in the interpreter.
fn define_checked_entry<M: Module>(
    module: &mut M,
    ctx: &mut cranelift_codegen::Context,
    body_id: FuncId,
    checks: &EntryChecks,
    num_params: usize,
) -> Option<FuncId> {
    let mut sig = module.make_signature();
    for _ in 0..num_params {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    let entry_id = module
        .declare_function("luna_jit_chunk_entry", Linkage::Local, &sig)
        .ok()?;
    let mut self_sig = module.make_signature();
    self_sig.params.push(AbiParam::new(types::I64));
    self_sig.returns.push(AbiParam::new(types::I64));
    let self_check_id = module
        .declare_function("luna_jit_self_upval_check", Linkage::Import, &self_sig)
        .ok()?;
    let mut math_sig = module.make_signature();
    math_sig.params.push(AbiParam::new(types::I64));
    math_sig.params.push(AbiParam::new(types::I64));
    math_sig.returns.push(AbiParam::new(types::I64));
    let math_check_id = module
        .declare_function("luna_jit_math_fn_is_library", Linkage::Import, &math_sig)
        .ok()?;
    let park_id = module
        .declare_function(
            "luna_jit_park_deopt",
            Linkage::Import,
            &module.make_signature(),
        )
        .ok()?;

    ctx.func.signature = sig;
    ctx.func.name = UserFuncName::user(0, entry_id.as_u32());
    let mut fbc = FunctionBuilderContext::new();
    let mut bcx = FunctionBuilder::new(&mut ctx.func, &mut fbc);
    let entry = bcx.create_block();
    let bail = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let args: Vec<Value> = bcx.block_params(entry).to_vec();
    // luna_jit_self_upval_check parks its own deopt; the math check
    // leaves that to the bail block.
    let park_on_bail = !checks.math_fns.is_empty();
    if let Some(idx) = checks.self_upval {
        let check_ref = module.declare_func_in_func(self_check_id, bcx.func);
        let idx = bcx.ins().iconst(types::I64, i64::from(idx));
        let call = bcx.ins().call(check_ref, &[idx]);
        let ok = bcx.inst_results(call)[0];
        let next = bcx.create_block();
        bcx.ins().brif(ok, next, &[], bail, &[]);
        bcx.switch_to_block(next);
    }
    for &(math_key, name_key) in &checks.math_fns {
        let check_ref = module.declare_func_in_func(math_check_id, bcx.func);
        let m = bcx.ins().iconst(types::I64, math_key.as_ptr() as i64);
        let k = bcx.ins().iconst(types::I64, name_key.as_ptr() as i64);
        let call = bcx.ins().call(check_ref, &[m, k]);
        let ok = bcx.inst_results(call)[0];
        let next = bcx.create_block();
        bcx.ins().brif(ok, next, &[], bail, &[]);
        bcx.switch_to_block(next);
    }
    let body_ref = module.declare_func_in_func(body_id, bcx.func);
    let call = bcx.ins().call(body_ref, &args);
    let r = bcx.inst_results(call)[0];
    bcx.ins().return_(&[r]);

    bcx.switch_to_block(bail);
    if park_on_bail {
        let park_ref = module.declare_func_in_func(park_id, bcx.func);
        bcx.ins().call(park_ref, &[]);
    }
    let zero = bcx.ins().iconst(types::I64, 0);
    bcx.ins().return_(&[zero]);

    bcx.seal_all_blocks();
    bcx.finalize(module.target_config());
    module.define_function(entry_id, ctx).ok()?;
    module.clear_context(ctx);
    Some(entry_id)
}

/// align a value with the Variable's declared Cranelift type
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
        bcx.ins().bitcast(want, MemFlagsData::new(), value)
    };
    bcx.def_var(regs[idx], aligned);
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
/// `_module` is typed as
/// [`SendJitModule`] (the `Send` sleeve newtype) so the module's
/// `Send` story stays type-system-asserted at this field. The
/// wrapper is a `#[repr(Rust)]` newtype with `Deref<Target = JITModule>`
/// + `DerefMut`, so existing call sites that touched
/// `handle._module.<method>` keep working transparently. The wrapper
/// also gates `Send` for any future container that wants to hold a
/// `JitHandle`; today the handle itself stays `!Send` because
/// `entry_raw: *const u8` is `!Send`; the manual `Send` impl below
/// builds on the module sleeve.
pub struct JitHandle {
    _module: SendJitModule,
    entry_raw: *const u8,
    /// Number of i64 args the entry expects (0..=MAX_JIT_ARITY).
    /// Picks the right `extern "C"` fn-type to transmute to at the
    /// call site.
    num_args: u8,
    /// True when the Lua chunk this fn was lowered from contains a
    /// `Return1`; false when only `Return0` is present. Drives the
    /// dispatch wrap (Int wrap vs empty Vec).
    returns_one: bool,
    /// bit `i = 1` ↔ arg slot `i` is f64 (passed as i64
    /// bit-pattern across the ABI, bitcast inside the JIT). Bits
    /// ≥ MAX_JIT_ARITY are always zero.
    arg_float_mask: u8,
    /// bit `i = 1` ↔ arg slot `i` is `Gc<Table>` raw ptr.
    /// Mutually exclusive with `arg_float_mask` for the same bit.
    arg_table_mask: u8,
    /// true iff the Proto's `Return1` value is f64.
    /// Meaningful only when `returns_one == true`.
    ret_is_float: bool,
    /// true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. Mutually exclusive with `ret_is_float`.
    ret_is_table: bool,
}

// sibling of the always-on
// `unsafe impl Send for TraceHandle` in `trace.rs`. JitHandle
// holds the same shape: a `SendJitModule` (Send via its wrapper,
// see `send_jit_module.rs`) plus an `entry_raw: *const u8` raw
// fn pointer addressing mcode owned by `_module`. The raw pointer
// is `!Send` by default — this manual impl is the explicit lift.
//
// SAFETY: each field is safely Send:
//   - `_module: SendJitModule` — Send via the `unsafe impl Send
//     for SendJitModule` in `send_jit_module.rs`. luna only
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
// Cross-thread dispatch is gated separately on the
// `scoped_jit_vm_rebind` RAII (per-`enter_jit` TLS install +
// restore), which works on any OS thread because the TLS slot is
// captured-and-restored at function scope rather than statically
// pinned.
unsafe impl Send for JitHandle {}

impl JitHandle {
    /// Frees the compiled code.
    ///
    /// # Safety
    ///
    /// The entry point is not running and will not be called again.
    pub(crate) unsafe fn free(self) {
        // SAFETY: forwarded from the caller
        unsafe { self._module.free() }
    }

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

    /// Raw entry fn ptr. The dispatcher stashes a copy in `Proto.jit` so the
    /// dispatch hot-path doesn't have to borrow back through the
    /// handle on every call. The handle itself stays parked in
    /// `Vm.jit_handles` to keep the mmap alive.
    #[inline]
    pub fn entry_raw(&self) -> *const u8 {
        self.entry_raw
    }

    /// `#[doc(hidden)]` accessor returning
    /// the parked `_module` borrowed at the `SendJitModule` newtype.
    /// Lets the regression test
    /// (`tests/it/jit_vm_scoped_rebind.rs`) statically assert the
    /// field type is the `Send` sleeve. The borrow checker enforces the
    /// type match at this fn's signature — if `_module` ever degrades
    /// to bare `JITModule` again, this signature stops compiling.
    #[doc(hidden)]
    #[inline]
    pub fn __send_module(&self) -> &SendJitModule {
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

    /// packed Float-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is f64 (the dispatcher passes `f64::to_bits` packed into the
    /// i64 ABI slot).
    #[inline]
    pub fn arg_float_mask(&self) -> u8 {
        self.arg_float_mask
    }

    /// true iff the Proto's `Return1` value is f64. The
    /// dispatcher wraps the i64 ABI return as `Value::Float(
    /// f64::from_bits(r))` when set, `Value::Int(r)` otherwise.
    #[inline]
    pub fn ret_is_float(&self) -> bool {
        self.ret_is_float
    }

    /// packed Table-arg mask. Bit `i = 1` ↔ arg slot `i`
    /// is `Gc<Table>` (the dispatcher passes the raw `as_ptr() as
    /// i64` value).
    #[inline]
    pub fn arg_table_mask(&self) -> u8 {
        self.arg_table_mask
    }

    /// true iff the Proto's `Return1` value is a
    /// `Gc<Table>` raw ptr. The dispatcher wraps the i64 ABI return
    /// as `Value::Table(Gc::from_ptr(r as *mut Table))`.
    #[inline]
    pub fn ret_is_table(&self) -> bool {
        self.ret_is_table
    }
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
mod chunk_tests_tables;
#[cfg(test)]
mod chunk_tests_setlist;

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
