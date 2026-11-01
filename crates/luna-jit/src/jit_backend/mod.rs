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
use luna_core::__private::{CompileOptions, CompiledTrace, TraceCompiler, TraceRecord};
use luna_core::jit::{CompileResult, IntChunkCompiler, JitVmGuard, MAX_JIT_ARITY};
use luna_core::runtime::Value as LuaValue;
use luna_core::runtime::function::Proto;
use luna_core::runtime::{Gc, LuaStr};
use luna_core::vm::isa::{Inst, Op};

use reg_kind::RegKind;

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
mod layout;
#[cfg(feature = "llvm-jit")]
mod llvm_backend;
#[cfg(feature = "llvm-jit")]
mod llvm_chunk;
#[cfg(feature = "llvm-jit")]
pub(crate) mod llvm_thread;
mod math_fold;
mod reg_kind;
mod send_jit_module;
mod trace_backend;
use getupval_roles::determine_getupval_roles;
#[cfg(feature = "llvm-jit")]
pub use llvm_backend::{LLVM_AFTER, LlvmBackend};
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
pub(crate) mod engine;
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
mod chunk_share;
pub use chunk_share::chunk_codegen_count;
mod chunk_lower;
mod chunk_module;
mod jit_handle;
pub(crate) use chunk_cache::CacheEntry;
pub use chunk_cache::{
    cache_clear, cache_entry_count, cache_lookup_or_compile, chunk_adopted_count,
};
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

pub(crate) use layout::*;

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

// SAFETY: each entry is a function Cranelift compiled from the prototype asked for,
// with the ABI the result states; its module stays in the Vm's JIT storage
unsafe impl IntChunkCompiler for CraneliftBackend {
    // pass storage through to
    // `cache_lookup_or_compile`; the cache lookup + handle park both
    // operate on `Vm.jit.storage.{cache,cache_handles}`.
    fn try_compile(
        &self,
        storage: &mut dyn luna_core::__private::JitStorage,
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
