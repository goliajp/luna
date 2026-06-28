//! v2.1 Phase 1K.D / 1K.E — LLVM int-chunk codegen.
//!
//! Two recognised paths land here:
//!
//! 1. **Dead-locals path** (Phase 1K.D.6 / 1K.D.7) —
//!    `[(LoadNil | LoadK | Move)*, Return0, ...]` chunks whose locals
//!    are unobservable at the `Return0` boundary. Emit shrinks to
//!    `extern "C" fn() -> i64 { ret 0 }` because no JIT-entry caller
//!    ever reads the locals (`returns_one = false`). This path covers
//!    chunks like `local x` / `local y = 'h'; local z = y` — including
//!    LoadK of *string* / *bool* / *nil* constants whose value the
//!    interpreter would compute but the JIT entry can elide.
//!
//! 2. **Compute path** (Phase 1K.E.2+) — chunks that reach an
//!    observable `Return1 R[A]`. The lowerer builds an `[N x i64]`
//!    register file on entry, emits one LLVM IR instruction per
//!    recognised op, and tails into either `ret i64 0` (Return0) or
//!    `ret <reg>` (Return1). The op whitelist grows incrementally —
//!    Phase 1K.E.2 covers `LoadI` + `Return0` + `Return1`; later
//!    sub-phases add `Move` / `LoadK_int` / `Add` / arith family /
//!    `Lt|Le|Eq` / `Jmp` / `LoadFalse|LoadTrue|LoadNil`.
//!
//! ## Why two paths instead of one
//!
//! The 1K.D dead-locals path can lower **chunks containing LoadK of
//! non-int constants** (string / bool / nil) because the values are
//! never observed. The 1K.E compute path can only lower ops whose
//! semantics it actually emits — so it bails on string / bool / nil
//! LoadK. Keeping both paths preserves the 1K.D.7 smoke coverage and
//! lets Phase 1K.E grow the whitelist op-by-op without breaking
//! previously-passing chunk shapes.
//!
//! ## Cache key
//!
//! Both paths share the FNV-1a-64-over-bytecode-words key (see
//! `proto_cache_key`). Constants are *not* fed in yet — the recognised
//! shapes either don't touch consts (LoadI/Move/arith on regs) or
//! treat them as unobservable (dead-locals LoadK). Phase 1K.E that
//! lights up `Add R, K` (constant operand) will widen the key.
//!
//! ## Lifetime path
//!
//! See [`super::storage`] module docs for the `'ctx` lifetime
//! reasoning. Each compile boxes a fresh `Context` and the engine
//! borrows it for `'static` via a localised pointer upgrade; the
//! pair lands in `LlvmJitStorage::engines` and gets dropped together
//! when the Vm drops.

use crate::storage::{CachedEntry, EnginePair, LlvmJitStorage};
use inkwell::OptimizationLevel;
use inkwell::basic_block::BasicBlock;
use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::values::{FunctionValue, PointerValue};
use luna_core::jit::{CompileResult, JitStorage};
use luna_core::runtime::{Gc, function::Proto};
use luna_core::vm::isa::{Inst, Op};
use std::collections::HashMap;
use std::hash::Hasher;

/// v2.1 Phase 1K.F.2 — `luna_jit_*` helper registry. Each tuple is
/// `(symbol_name, fn_ptr_as_usize, n_args, returns_i64)`. The full
/// 27-entry set from `luna-jit-helpers` lives here so future ops can
/// reach for any helper without re-touching the registration path;
/// the inkwell `add_function` + `ExecutionEngine::add_global_mapping`
/// pair fires unconditionally on every compute-path compile. LLVM
/// will strip the unreferenced declarations at JIT time (zero cost
/// when only a subset is actually called).
///
/// Pointer-typed helper args (`*const u8`, `*mut i64`, …) are declared
/// as `i64` because:
/// 1. luna-jit-helpers' own bodies bit-cast `i64` → `*const _` at
///    the boundary; Cranelift's `build_jit_module_with_helpers`
///    mirrors this (all `AbiParam::new(types::I64)`).
/// 2. LLVM 16+ uses opaque pointers — passing `i64` through the
///    C-ABI and bitcast'ing on the helper side is layout-equivalent
///    to passing a `ptr` directly on 64-bit targets.
/// 3. Keeps the registry one-liner per helper.
fn helper_registry() -> Vec<(&'static str, usize, u32, bool)> {
    use luna_jit_helpers::{
        luna_jit_materialize_sunk_table, luna_jit_new_table, luna_jit_new_table_sized,
        luna_jit_op_close, luna_jit_op_closure, luna_jit_op_concat, luna_jit_op_get_tab_up,
        luna_jit_op_tforcall, luna_jit_spill_to_stack, luna_jit_stack_load, luna_jit_stack_tag,
        luna_jit_stack_update_raw, luna_jit_str_buf_acquire, luna_jit_str_buf_extend,
        luna_jit_str_buf_intern, luna_jit_str_buf_release, luna_jit_table_get_field,
        luna_jit_table_get_float, luna_jit_table_get_int, luna_jit_table_len,
        luna_jit_table_set_field, luna_jit_table_set_float_float, luna_jit_table_set_int,
        luna_jit_table_set_nil, luna_jit_table_set_raw, luna_jit_trace_materialize_frames,
        luna_jit_upval_get,
    };
    vec![
        (
            "luna_jit_new_table",
            luna_jit_new_table as *const () as usize,
            0,
            true,
        ),
        (
            "luna_jit_new_table_sized",
            luna_jit_new_table_sized as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_materialize_sunk_table",
            luna_jit_materialize_sunk_table as *const () as usize,
            7,
            true,
        ),
        (
            "luna_jit_table_set_int",
            luna_jit_table_set_int as *const () as usize,
            3,
            false,
        ),
        (
            "luna_jit_table_set_raw",
            luna_jit_table_set_raw as *const () as usize,
            4,
            false,
        ),
        (
            "luna_jit_table_set_field",
            luna_jit_table_set_field as *const () as usize,
            4,
            false,
        ),
        (
            "luna_jit_table_get_field",
            luna_jit_table_get_field as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_op_get_tab_up",
            luna_jit_op_get_tab_up as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_table_set_nil",
            luna_jit_table_set_nil as *const () as usize,
            2,
            false,
        ),
        (
            "luna_jit_table_set_float_float",
            luna_jit_table_set_float_float as *const () as usize,
            3,
            false,
        ),
        (
            "luna_jit_table_get_int",
            luna_jit_table_get_int as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_table_get_float",
            luna_jit_table_get_float as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_upval_get",
            luna_jit_upval_get as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_op_close",
            luna_jit_op_close as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_stack_update_raw",
            luna_jit_stack_update_raw as *const () as usize,
            2,
            false,
        ),
        (
            "luna_jit_op_concat",
            luna_jit_op_concat as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_str_buf_acquire",
            luna_jit_str_buf_acquire as *const () as usize,
            0,
            true,
        ),
        (
            "luna_jit_str_buf_release",
            luna_jit_str_buf_release as *const () as usize,
            1,
            false,
        ),
        (
            "luna_jit_str_buf_extend",
            luna_jit_str_buf_extend as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_str_buf_intern",
            luna_jit_str_buf_intern as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_op_tforcall",
            luna_jit_op_tforcall as *const () as usize,
            5,
            true,
        ),
        (
            "luna_jit_stack_load",
            luna_jit_stack_load as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_stack_tag",
            luna_jit_stack_tag as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_spill_to_stack",
            luna_jit_spill_to_stack as *const () as usize,
            3,
            false,
        ),
        (
            "luna_jit_op_closure",
            luna_jit_op_closure as *const () as usize,
            1,
            true,
        ),
        (
            "luna_jit_trace_materialize_frames",
            luna_jit_trace_materialize_frames as *const () as usize,
            2,
            true,
        ),
        (
            "luna_jit_table_len",
            luna_jit_table_len as *const () as usize,
            1,
            true,
        ),
    ]
}

/// v2.1 Phase 1K.F.2 — declare every `luna_jit_*` helper in `module`
/// as an external function. The returned map lets emit sites grab the
/// `FunctionValue` for `build_call` without re-looking-up via
/// `Module::get_function` per call. Pairs with [`bind_helper_symbols`]
/// which fires after `create_jit_execution_engine` to wire the IR
/// declarations to their actual Rust function addresses.
/// v2.1 Phase 1K.G — also used by the trace JIT (`trace.rs`).
pub(crate) fn declare_jit_helpers<'ctx>(
    ctx: &'ctx Context,
    module: &Module<'ctx>,
) -> HashMap<&'static str, FunctionValue<'ctx>> {
    let i64_ty = ctx.i64_type();
    let void_ty = ctx.void_type();
    let mut map = HashMap::with_capacity(27);
    for (name, _addr, n_args, returns_i64) in helper_registry() {
        let params: Vec<inkwell::types::BasicMetadataTypeEnum> =
            (0..n_args).map(|_| i64_ty.into()).collect();
        let fn_ty = if returns_i64 {
            i64_ty.fn_type(&params, false)
        } else {
            void_ty.fn_type(&params, false)
        };
        let fn_val = module.add_function(name, fn_ty, Some(Linkage::External));
        map.insert(name, fn_val);
    }
    map
}

/// v2.1 Phase 1K.F.2 — bind every declared `luna_jit_*` helper to its
/// Rust function address via `ExecutionEngine::add_global_mapping`.
/// Without this step the JIT'd mcode would call undefined external
/// symbols at run time (LLVM's default resolver tries `dlsym` first,
/// and luna's `unsafe extern "C"` helpers are `#[unsafe(no_mangle)]`
/// so they ARE dlsym-able when luna is loaded as a `dylib`/`cdylib`
/// — but the rlib link path strips them, exactly mirroring Cranelift's
/// `JITBuilder::symbol` rationale in `build_jit_module_with_helpers`).
/// v2.1 Phase 1K.G — also used by the trace JIT (`trace.rs`).
pub(crate) fn bind_helper_symbols<'ctx>(
    engine: &inkwell::execution_engine::ExecutionEngine<'ctx>,
    helpers: &HashMap<&'static str, FunctionValue<'ctx>>,
) {
    for (name, addr, _n_args, _returns_i64) in helper_registry() {
        if let Some(fn_val) = helpers.get(name) {
            engine.add_global_mapping(fn_val, addr);
        }
    }
}

/// v2.1 Phase 1K.D.7 / 1K.E.2 — try to lower `proto` to native code
/// via LLVM. Returns `None` when the body falls outside the recognised
/// shape (caller turns into `CompileResult::Skipped`).
///
/// Recognised shapes:
/// - **Dead locals** (Phase 1K.D): a (possibly empty) prefix of
///   `LoadNil | LoadK | Move` followed by `Return0`. Lowers to
///   `extern "C" fn() -> i64 { ret 0 }`.
/// - **Compute** (Phase 1K.E.2): a (possibly empty) prefix of
///   `LoadI | LoadNil | LoadK(Int) | Move` followed by either
///   `Return1` or `Return0`. Lowers to a per-op reg-array entry.
///
/// `pre53` is fed into the cache key for ABI parity with Cranelift's
/// `proto_cache_key`; the recognised shapes currently don't touch the
/// dialect-bit semantics.
pub(crate) fn try_compile_int_chunk(
    storage: &mut dyn JitStorage,
    proto: Gc<Proto>,
    pre53: bool,
) -> Option<CompileResult> {
    // Path 1: dead-locals fast path (Phase 1K.D.7). Restricted to
    // zero-param chunks because its `extern "C" fn() -> i64` emit
    // signature doesn't accept positional args.
    if proto.num_params == 0 && is_dead_locals_then_return0(&proto.code) {
        return Some(via_cache(storage, &proto, pre53, 0, false, &|| {
            compile_constant_zero_chunk()
        }));
    }

    // Path 2: compute path (Phase 1K.E.2+). Scans for the cumulative
    // whitelist + the chunk's effective return shape, then lowers
    // op-by-op into a reg-array entry. Phase 1K.F lifted the
    // `num_params == 0` gate; parametric chunks land as
    // `extern "C" fn(i64, …, i64) -> i64` with arg-load shims in the
    // entry BB.
    if let Some(plan) = ChunkPlan::from_proto(&proto) {
        let num_params = plan.num_params as u8;
        return Some(via_cache(
            storage,
            &proto,
            pre53,
            num_params,
            plan.returns_one,
            &|| compile_compute_chunk(&plan),
        ));
    }

    None
}

/// Shared cache+compile wrapper. Looks up the proto in the storage
/// cache; on a hit returns the cached entry, on a miss invokes
/// `compile_fn`, parks the resulting `(EngineEntry, EnginePair)` on
/// the cache, and returns the new entry. `returns_one` is baked into
/// the `CachedEntry` so a hit on a subsequent compile reproduces the
/// same dispatcher contract.
fn via_cache(
    storage: &mut dyn JitStorage,
    proto: &Proto,
    pre53: bool,
    num_args: u8,
    returns_one: bool,
    compile_fn: &dyn Fn() -> Option<(*const u8, EnginePair)>,
) -> CompileResult {
    let store = storage
        .as_any_mut()
        .downcast_mut::<LlvmJitStorage>()
        .expect("LlvmBackend installed without LlvmJitStorage");
    let key = proto_cache_key(proto, pre53);
    if let Some(hit) = store.cache.get(&key).copied() {
        return hit.to_compile_result();
    }
    let Some((entry_ptr, pair)) = compile_fn() else {
        return CompileResult::Skipped;
    };
    let cached = CachedEntry {
        entry: entry_ptr,
        num_args,
        returns_one,
        arg_float_mask: 0,
        arg_table_mask: 0,
        ret_is_float: false,
        ret_is_table: false,
    };
    store.insert(key, pair, cached);
    cached.to_compile_result()
}

/// Phase 1K.D.7 / 1K.E.7 — accept
/// `[(LoadNil | LoadK | Move | LoadFalse | LoadTrue)*, Return0, ...]`.
///
/// The dead-locals path eats every load op whose effect is invisible
/// at the `Return0` boundary, regardless of what value the load
/// produces — strings, booleans, nils, and any LoadK constant kind
/// all qualify because the chunk returns no value. The trailing
/// implicit `Return0` the parser emits after a `Return1` is not
/// relevant here — we only fire on a chunk whose *first* reachable
/// return is `Return0`.
///
/// `LoadFalse` / `LoadTrue` are accepted on the dead-locals path
/// **only**; the compute path needs a dispatcher widening
/// (`ret_is_bool` bit) before `return true` / `return false` can
/// flow through the JIT-entry → caller contract without misreading
/// the i64 as an integer. That widening is part of a future
/// sub-phase (paired with `LoadF` / float-return support); until
/// then a chunk like `return true` falls through to the interpreter.
fn is_dead_locals_then_return0(code: &[Inst]) -> bool {
    let Some(first_ret_pc) = code
        .iter()
        .position(|i| matches!(i.op(), Op::Return0 | Op::Return1))
    else {
        return false;
    };
    if code[first_ret_pc].op() != Op::Return0 {
        return false;
    }
    code[..first_ret_pc].iter().all(|i| {
        matches!(
            i.op(),
            Op::LoadNil | Op::LoadK | Op::Move | Op::LoadFalse | Op::LoadTrue
        )
    })
}

/// Phase 1K.E.2+ — whitelisted op set + control-flow plan that the
/// compute lowerer understands. Built by [`ChunkPlan::from_proto`];
/// `None` when the proto falls outside the supported whitelist.
///
/// The plan is the full reach-analysed bytecode + a per-PC vector of
/// basic-block start markers (every entry PC, every jump target, every
/// PC immediately following a terminator). With branching ops in scope
/// (Phase 1K.E.5+6) "reachable" is no longer "sequential prefix"; we
/// trace edges from PC 0 and mark every visited PC for emit.
struct ChunkPlan<'a> {
    /// Full chunk code (not truncated). The reach map (`reachable`)
    /// tells the lowerer which PCs to emit; unreachable PCs are
    /// skipped entirely.
    code: &'a [Inst],
    /// Number of i64 register slots to alloca on entry.
    num_regs: u32,
    /// v2.1 Phase 1K.F.6 — number of positional i64 args the JIT entry
    /// accepts. `0` keeps the historical `extern "C" fn() -> i64`
    /// signature; `> 0` widens to `fn(i64, …, i64) -> i64` and the
    /// entry BB loads each `function.get_nth_param(i)` into `regs[i]`
    /// so the lowerer sees param 0..N-1 as live register sources.
    num_params: u32,
    /// True ↔ all reachable returns are `Return1`; false ↔ all are
    /// `Return0`. A proto whose reachable set mixes the two bails
    /// (would need a polymorphic dispatcher contract — out of scope).
    returns_one: bool,
    /// `true` at every PC that starts a new basic block. PC 0 always
    /// starts a BB; jump targets, the PC immediately after every
    /// terminator (`Return0|Return1|Jmp`), and the fall-through PC
    /// after a `Lt|Le|Eq` (which is `pc+2` because the paired Jmp at
    /// `pc+1` is consumed by the condbr) all start BBs too.
    bb_starts: Vec<bool>,
    /// `true` at every PC that holds a `Jmp` consumed by a preceding
    /// `Lt|Le|Eq` (i.e. the Jmp is folded into the condbr emit and
    /// must not be lowered as a separate op).
    consumed_jmp: Vec<bool>,
    /// `true` at every PC reachable via the control-flow trace from
    /// PC 0. Unreachable PCs (e.g. the trailing implicit `Return0`
    /// after every reachable path has already returned) are skipped
    /// during emit so LLVM doesn't see dead BBs without predecessors.
    reachable: Vec<bool>,
    /// v2.1 Phase 1K.F.3 — `true` at every `Op::Call` PC the scanner
    /// classified as a self-recursive call (R[A] tagged as a
    /// self-upval-loaded closure via a prior `Op::GetUpval` in the
    /// SelfMarker role). The Call lowerer emits a direct `build_call`
    /// to the current entry `FunctionValue` for these PCs.
    self_call_pcs: Vec<bool>,
    /// v2.1 Phase 1K.G — `true` at every `Op::TailCall` PC the scanner
    /// classified as a self-recursive tail call. Emit: `build_call
    /// (function, args) → ret result` (Op::Call + Op::Return1 fused).
    /// Treated as a terminator: no BB successor, implies returns_one.
    tail_call_pcs: Vec<bool>,
    /// v2.1 Phase 1K.F.4 — `true` at every `Op::GetUpval` PC whose
    /// destination register is consumed as a runtime value (i.e. NOT
    /// used as the function slot of a subsequent `Op::Call`). Emitter
    /// lowers these as `luna_jit_upval_get(b as i64) -> i64`.
    /// `false` at GetUpval PCs whose role is SelfMarker — emitter
    /// writes a placeholder 0 because the matching Call rewrites
    /// straight to `build_call(function, …)` without reading R[A].
    is_upval_value_read: Vec<bool>,
}

impl<'a> ChunkPlan<'a> {
    fn from_proto(proto: &'a Proto) -> Option<Self> {
        let code: &'a [Inst] = &proto.code;
        let n = code.len();
        if n == 0 {
            return None;
        }
        // Parametric chunks need a stable fn signature. Cap at the
        // Cranelift `MAX_JIT_ARITY` (16) so the dispatcher's
        // `JitHandle` codepath stays interchangeable across backends.
        const MAX_JIT_ARITY: u32 = 16;
        if proto.num_params as u32 > MAX_JIT_ARITY {
            return None;
        }

        // Pass 1: per-op whitelist gate + structural validation.
        // `Lt|Le|Eq` must be followed by a `Jmp`; mark the Jmp as
        // consumed by the condbr.
        //
        // 1K.F additions: `Op::GetUpval` (both SelfMarker + ValueRead
        // roles, classified in a subsequent pass) and `Op::Call`
        // restricted to self-recursive shapes (validated in the
        // self_upval tracking pass below).
        let mut consumed_jmp = vec![false; n];
        for (pc, ins) in code.iter().enumerate() {
            match ins.op() {
                Op::LoadI
                | Op::LoadNil
                | Op::Move
                | Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Mod
                | Op::Jmp
                | Op::Return0
                | Op::Return1 => {}
                Op::GetUpval => {
                    // Upval idx must be in bounds. The self-rec /
                    // value-read classification + Float-tag concerns
                    // are handled by the later
                    // `determine_getupval_roles` + tracking pass.
                    if (ins.b() as usize) >= proto.upvals.len() {
                        return None;
                    }
                }
                Op::Call => {
                    // The structural gate enforced here mirrors
                    // Cranelift's `Op::Call` admission: nargs / nresults
                    // bounds. The "self-recursive" classification
                    // (mandatory in 1K.F) needs the SelfMarker upval
                    // tracking; done in the dedicated pass below.
                    let nargs = ins.b().checked_sub(1)?;
                    let nresults = ins.c().checked_sub(1)?;
                    if nargs > MAX_JIT_ARITY || nresults != 1 {
                        return None;
                    }
                }
                Op::TailCall => {
                    // 1K.G — TailCall has B-1 args, no explicit nresults
                    // (it returns whatever the called fn returns to OUR
                    // caller). Same nargs bound as Op::Call.
                    let nargs = ins.b().checked_sub(1)?;
                    if nargs > MAX_JIT_ARITY {
                        return None;
                    }
                }
                Op::Lt | Op::Le | Op::Eq => {
                    let peer = code.get(pc + 1)?;
                    if peer.op() != Op::Jmp {
                        return None;
                    }
                    consumed_jmp[pc + 1] = true;
                }
                _ => return None,
            }
        }

        // Pre-pass — classify each `Op::GetUpval` PC as ValueRead vs
        // SelfMarker. Mirrors Cranelift's `determine_getupval_roles`:
        // a SelfMarker is one whose destination register is consumed
        // ONLY as the function slot of a subsequent `Op::Call` (within
        // a short lookahead window, with Moves carrying the tag).
        // Anything else (arith / cmp / Return / table-access read of
        // R[A]) is a ValueRead.
        let is_upval_value_read = determine_getupval_roles(code);

        // Self-recursion tracking pass: walk PCs in order, maintain
        // `self_upval[reg]` (cleared on any write that doesn't carry
        // the tag), gate each `Op::Call` to the self-recursive shape.
        // Also pins `self_upval_idx` to the SelfMarker GetUpval's
        // upval slot — subsequent SelfMarker GetUpvals must read the
        // same slot, else bail. Mirrors Cranelift's S2c.C tracker.
        let allows_self_recursion = !proto.upvals.is_empty() && proto.upvals.len() <= 4;
        let mut self_upval_idx: Option<u32> = None;
        let max_stack = (proto.max_stack as usize).max(proto.num_params as usize);
        let mut self_upval: Vec<bool> = vec![false; max_stack];
        let mut self_call_pcs: Vec<bool> = vec![false; n];
        let mut tail_call_pcs: Vec<bool> = vec![false; n];
        for (pc, ins) in code.iter().enumerate() {
            // Apply writes: any op that writes R[A] clears
            // `self_upval[A]` unless it's the SelfMarker GetUpval (which
            // sets the tag) or a Move from another self_upval-tagged
            // slot (which carries the tag).
            match ins.op() {
                Op::GetUpval => {
                    if is_upval_value_read[pc] {
                        // ValueRead clears the dest tag — the loaded
                        // value is consumed as a real upvalue value,
                        // not a self-recursion call target.
                        if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                            *slot = false;
                        }
                    } else {
                        // SelfMarker — must agree with any prior
                        // SelfMarker on the upval index.
                        if !allows_self_recursion {
                            return None;
                        }
                        match self_upval_idx {
                            Some(idx) if idx != ins.b() => return None,
                            Some(_) => {}
                            None => self_upval_idx = Some(ins.b()),
                        }
                        if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                            *slot = true;
                        }
                    }
                }
                Op::Move => {
                    let src = ins.b() as usize;
                    let dst = ins.a() as usize;
                    let carry = self_upval.get(src).copied().unwrap_or(false);
                    if let Some(slot) = self_upval.get_mut(dst) {
                        *slot = carry;
                    }
                }
                Op::Call => {
                    let a = ins.a() as usize;
                    if self_upval.get(a).copied().unwrap_or(false) {
                        self_call_pcs[pc] = true;
                    } else {
                        // 1K.F restricts Op::Call to self-recursive
                        // shapes; non-self calls would need a general
                        // ABI marshalling shim (Cranelift bails the
                        // same way at this layer).
                        return None;
                    }
                    // The Call writes R[A] = result; result is the
                    // self-rec return value, not a closure handle.
                    if let Some(slot) = self_upval.get_mut(a) {
                        *slot = false;
                    }
                }
                Op::TailCall => {
                    // 1K.G — same self-recursion gate as Op::Call.
                    // TailCall R[A] is the function slot; it must be
                    // tagged as the self-upval closure for our direct
                    // `build_call` to be sound (otherwise we'd need a
                    // general call ABI which is out of scope here).
                    let a = ins.a() as usize;
                    if self_upval.get(a).copied().unwrap_or(false) {
                        tail_call_pcs[pc] = true;
                    } else {
                        return None;
                    }
                    // TailCall is a tail return — it does NOT write R[A]
                    // (there's no subsequent op that would read the
                    // result). No tag update needed for self_upval.
                }
                Op::Return1 => {
                    // Returning a self_upval-tagged register would
                    // ship the closure pointer back to the caller,
                    // which our int-only dispatcher would misread as
                    // an integer; bail.
                    if self_upval.get(ins.a() as usize).copied().unwrap_or(false) {
                        return None;
                    }
                }
                _ => {
                    // Other ops clear the tag for any register they
                    // write. For the present whitelist that's: LoadI
                    // / LoadNil / Add / Sub / Mul / Mod — all write
                    // arithmetic / immediate values that can't be a
                    // closure pointer.
                    let writes: &[u32] = match ins.op() {
                        Op::LoadI | Op::Add | Op::Sub | Op::Mul | Op::Mod => &[ins.a()],
                        // LoadNil writes R[A..=A+B]; clear them.
                        Op::LoadNil => {
                            for off in 0..=ins.b() {
                                if let Some(slot) = self_upval.get_mut((ins.a() + off) as usize) {
                                    *slot = false;
                                }
                            }
                            &[]
                        }
                        _ => &[],
                    };
                    for &w in writes {
                        if let Some(slot) = self_upval.get_mut(w as usize) {
                            *slot = false;
                        }
                    }
                }
            }
        }

        // Pass 2: reach analysis from PC 0. A worklist trace
        // following the control-flow edges; terminators have no
        // successor.
        let mut reachable = vec![false; n];
        let mut worklist = vec![0usize];
        while let Some(pc) = worklist.pop() {
            if pc >= n || reachable[pc] {
                continue;
            }
            reachable[pc] = true;
            let ins = code[pc];
            match ins.op() {
                Op::Return0 | Op::Return1 | Op::TailCall => {} // terminator, no successor
                Op::Jmp => {
                    if consumed_jmp[pc] {
                        // Consumed by the preceding Lt|Le|Eq; the
                        // edges from this Jmp are folded into the
                        // comparison's condbr. Visiting it again
                        // here would mark it reachable as a
                        // standalone op, which is wrong (the emit
                        // loop skips consumed Jmps).
                        continue;
                    }
                    worklist.push(jmp_target(pc, ins));
                }
                Op::Lt | Op::Le | Op::Eq => {
                    // The peer Jmp at pc+1 supplies the false-edge
                    // target; pc+2 is the true-edge (skip-next) fall.
                    worklist.push(pc + 2);
                    if let Some(jmp) = code.get(pc + 1) {
                        worklist.push(jmp_target(pc + 1, *jmp));
                    }
                }
                _ => worklist.push(pc + 1),
            }
        }

        // Pass 3: returns_one analysis. Every reachable Return* must
        // agree on shape (Return0 or Return1) so the dispatcher
        // contract has a single answer. Op::TailCall is treated as
        // Return1 (it returns the called function's result = one value).
        let mut found: Option<bool> = None;
        for (pc, ins) in code.iter().enumerate() {
            if !reachable[pc] {
                continue;
            }
            match ins.op() {
                Op::Return0 => match found {
                    Some(true) => return None,
                    _ => found = Some(false),
                },
                Op::Return1 | Op::TailCall => match found {
                    Some(false) => return None,
                    _ => found = Some(true),
                },
                _ => {}
            }
        }
        let returns_one = found?;

        // Pass 4: BB starts. PC 0 always; jump targets; every PC
        // immediately after a terminator; the fall-through PC after
        // a `Lt|Le|Eq` (pc+2 — the consumed Jmp at pc+1 is folded).
        let mut bb_starts = vec![false; n];
        bb_starts[0] = true;
        for (pc, ins) in code.iter().enumerate() {
            if !reachable[pc] {
                continue;
            }
            match ins.op() {
                Op::Lt | Op::Le | Op::Eq => {
                    if pc + 2 < n {
                        bb_starts[pc + 2] = true;
                    }
                    if let Some(jmp) = code.get(pc + 1) {
                        let tgt = jmp_target(pc + 1, *jmp);
                        if tgt < n {
                            bb_starts[tgt] = true;
                        }
                    }
                }
                Op::Jmp if !consumed_jmp[pc] => {
                    let tgt = jmp_target(pc, *ins);
                    if tgt < n {
                        bb_starts[tgt] = true;
                    }
                    if pc + 1 < n {
                        bb_starts[pc + 1] = true;
                    }
                }
                Op::Return0 | Op::Return1 | Op::TailCall if pc + 1 < n => {
                    bb_starts[pc + 1] = true;
                }
                _ => {}
            }
        }

        // Register-bounds sanity check on every reachable op that
        // names a slot. `proto.max_stack` is the upper bound the
        // parser guarantees; an out-of-range A would write past the
        // alloca, so bail rather than corrupt memory.
        let regs = (proto.max_stack as u32).max(proto.num_params as u32).max(1);
        for (pc, ins) in code.iter().enumerate() {
            if !reachable[pc] {
                continue;
            }
            let max_slot = match ins.op() {
                Op::LoadI | Op::Move | Op::Return1 | Op::GetUpval => ins.a(),
                Op::LoadNil => ins.a() + ins.b(),
                Op::Add | Op::Sub | Op::Mul | Op::Mod => ins.a().max(ins.b()).max(ins.c()),
                Op::Lt | Op::Le | Op::Eq => ins.a().max(ins.b()),
                Op::Call | Op::TailCall => {
                    // Op::Call/TailCall read R[A] (function) and
                    // R[A+1..A+nargs] (args); A is the max slot.
                    let nargs = ins.b().saturating_sub(1);
                    if nargs == 0 { ins.a() } else { ins.a() + nargs }
                }
                _ => 0,
            };
            if max_slot >= regs {
                return None;
            }
        }

        Some(ChunkPlan {
            code,
            num_regs: regs,
            num_params: proto.num_params as u32,
            returns_one,
            bb_starts,
            consumed_jmp,
            reachable,
            self_call_pcs,
            tail_call_pcs,
            is_upval_value_read,
        })
    }
}

/// v2.1 Phase 1K.F.4 — classify every `Op::GetUpval` in `code` as
/// either a SelfMarker (its destination register is consumed only as
/// the function slot of a subsequent `Op::Call`) or a ValueRead (the
/// register is consumed as a real value — arith, comparison, return,
/// etc.). Mirrors Cranelift's `determine_getupval_roles` algorithm.
///
/// The classifier walks an 8-PC lookahead from each `GetUpval`,
/// tracking `Move`-carrying tags on each register. If the destination
/// register reaches an `Op::Call` whose function-slot register is the
/// tagged one, it's SelfMarker. Any other consuming op flips it to
/// ValueRead. The lookahead is bounded to keep the scanner O(N).
fn determine_getupval_roles(code: &[Inst]) -> Vec<bool> {
    let n = code.len();
    let mut roles = vec![false; n];
    // 8-PC lookahead window, matching Cranelift S2c.C scanner.
    const LOOKAHEAD: usize = 8;
    for (pc, ins) in code.iter().enumerate() {
        if !matches!(ins.op(), Op::GetUpval) {
            continue;
        }
        let target_a = ins.a() as usize;
        // Per-register "carries the marker" map for this lookahead.
        let max_reg = code
            .iter()
            .map(|i| i.a().max(i.b()).max(i.c()))
            .max()
            .unwrap_or(0) as usize
            + 1;
        let mut tagged: Vec<bool> = vec![false; max_reg.max(target_a + 1)];
        tagged[target_a] = true;
        let end = (pc + 1 + LOOKAHEAD).min(n);
        let mut value_read = false;
        for q in (pc + 1)..end {
            let q_ins = code[q];
            // Op::Call with R[A] as the function slot — confirmed SelfMarker.
            if matches!(q_ins.op(), Op::Call)
                && tagged.get(q_ins.a() as usize).copied().unwrap_or(false)
            {
                // SelfMarker: leave roles[pc] = false.
                value_read = false;
                break;
            }
            // Move from tagged src to dst: carry the tag.
            if matches!(q_ins.op(), Op::Move) {
                let src = q_ins.b() as usize;
                let dst = q_ins.a() as usize;
                if dst >= tagged.len() {
                    tagged.resize(dst + 1, false);
                }
                let carry = tagged.get(src).copied().unwrap_or(false);
                tagged[dst] = carry;
                continue;
            }
            // Any op that reads a tagged register in a non-Call /
            // non-Move context confirms ValueRead.
            if uses_register_as_value(&q_ins, &tagged) {
                value_read = true;
                break;
            }
            // Any write to a tagged register clears the tag (the
            // dest no longer carries the marker).
            if let Some(write_reg) = primary_write_reg(&q_ins) {
                if let Some(slot) = tagged.get_mut(write_reg as usize) {
                    *slot = false;
                }
            }
        }
        roles[pc] = value_read;
    }
    roles
}

/// Helper for `determine_getupval_roles`: returns `true` if `ins`
/// reads any register currently flagged in `tagged` as a value-use
/// (NOT as the function slot of an `Op::Call`).
fn uses_register_as_value(ins: &Inst, tagged: &[bool]) -> bool {
    let is_tagged = |idx: u32| tagged.get(idx as usize).copied().unwrap_or(false);
    match ins.op() {
        Op::Return1 => is_tagged(ins.a()),
        Op::Add | Op::Sub | Op::Mul | Op::Mod => is_tagged(ins.b()) || is_tagged(ins.c()),
        Op::Lt | Op::Le | Op::Eq => is_tagged(ins.a()) || is_tagged(ins.b()),
        // Op::Call / Op::TailCall args (R[A+1..A+nargs]) are value uses.
        Op::Call | Op::TailCall => {
            let nargs = ins.b().saturating_sub(1);
            for off in 1..=nargs {
                if is_tagged(ins.a() + off) {
                    return true;
                }
            }
            false
        }
        // Move is handled separately by the caller (it carries the
        // tag rather than confirming ValueRead).
        Op::Move => false,
        _ => false,
    }
}

/// Helper for `determine_getupval_roles`: returns the primary write
/// destination register for the supported op set. Used to clear the
/// tagged marker when a register is overwritten.
fn primary_write_reg(ins: &Inst) -> Option<u32> {
    match ins.op() {
        Op::LoadI
        | Op::LoadNil
        | Op::Move
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Mod
        | Op::GetUpval
        | Op::Call => Some(ins.a()),
        _ => None,
    }
}

/// Lua `Jmp` target: `(pc + 1) + sj`. Matches
/// `luna_jit::jit_backend::jmp_target`. Returns a `usize` — the caller
/// (ChunkPlan / compile_compute_chunk) bounds-checks against `n`
/// before using it as an index.
fn jmp_target(pc: usize, ins: Inst) -> usize {
    (pc as i64 + 1 + ins.sj() as i64) as usize
}

// Compute-path whitelist roadmap (consumption itself happens inside
// `ChunkPlan::from_proto`):
//
// - 1K.E.2: `LoadI` + `LoadNil` + `Move`. Cache key over bytecode
//   words is sufficient — the recognised ops don't touch
//   `proto.consts`.
// - 1K.E.3: `Add`. Still no consts; key stays as-is.
// - 1K.E.4: `Sub` / `Mul` / `Mod` (Lua semantics — floor mod, sign
//   matches divisor — not C's truncating srem). `Op::Div` is
//   intentionally **excluded** because Lua 5.4 `/` always returns a
//   float regardless of operand types; emitting it as int sdiv would
//   silently mis-compile `2 / 3` (Lua → 0.666…, the int chunk would
//   return 0). Div lands paired with float-reg support
//   (`ret_is_float=true` + `f64::from_bits` reinterpret).
// - 1K.E.5+6: `Lt` / `Le` / `Eq` + `Jmp`. Comparison-then-jmp becomes
//   a single LLVM `condbr`; bare `Jmp` becomes `br`. Multiple
//   reachable returns are tolerated (must agree on
//   `Return0`-vs-`Return1` shape).
// - 1K.E.7: `LoadFalse` / `LoadTrue` + `LoadK(Int)` (the `LoadK(Int)`
//   add widens the cache key to include the constant table).
// - 1K.E.7+: float support — `LoadF` / `Op::Div` / float-only chunks
//   (`ret_is_float=true`).

/// Phase 1K.D.8 / 1K.E.2 — stable cache key for a Proto. FNV-1a-64
/// over the bytecode words + the dialect bit. Constants are not fed
/// in yet (Phase 1K.E that lights up `Add R, K` will widen the key).
fn proto_cache_key(proto: &Proto, pre53: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for inst in proto.code.iter() {
        h.write_u32(inst.0);
    }
    h.write_u8(pre53 as u8);
    h.finish()
}

/// Build the dead-locals JIT entry: `extern "C" fn() -> i64 { ret 0 }`.
/// Returns the entry pointer + owning `(Context, ExecutionEngine)`
/// pair so the caller can park it on storage. Used by the Phase 1K.D
/// fast path; the Phase 1K.E compute path uses [`compile_compute_chunk`].
fn compile_constant_zero_chunk() -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: see `compile_compute_chunk` below for the full lifetime
    // discussion; the same reasoning applies — ctx_box outlives the
    // engine via `EnginePair`'s field-order drop discipline.
    let ctx_static: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };

    let module = ctx_static.create_module("luna_jit_llvm_dead_locals");
    let builder = ctx_static.create_builder();

    let i64_type = ctx_static.i64_type();
    let fn_type = i64_type.fn_type(&[], false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);
    let entry_block = ctx_static.append_basic_block(function, "entry");
    builder.position_at_end(entry_block);
    let zero = i64_type.const_int(0, false);
    builder.build_return(Some(&zero)).ok()?;

    finalize_module(ctx_box, module, None)
}

/// v2.1 Phase 1K.E.2+ — lower a compute-path chunk into a JIT entry.
///
/// Emit shape (Phase 1K.E.5+6 with control flow):
/// ```text
/// extern "C" fn luna_jit_llvm_entry() -> i64 {
///     bb_0:                              ; entry — alloca + sequential IR
///         %regs = alloca [N x i64]       ; N = plan.num_regs
///         ; per-op IR for each reachable PC in BB 0
///         br bb_<jump-target>            ; or condbr / ret
///     bb_<pc>:                           ; one LLVM BB per ChunkPlan::bb_starts[pc]
///         ; per-op IR for each reachable PC in this BB
///         br / condbr / ret              ; terminator
///     ...
/// }
/// ```
///
/// Per-PC emit:
/// - `LoadI rA, sBx`     → store i64 sBx, regs[A]
/// - `LoadNil rA, B`     → store i64 0 for regs[A..=A+B]
/// - `Move rA, rB`       → load regs[B]; store regs[A]
/// - `Add|Sub|Mul rA,rB,rC` → load regs[B]; load regs[C]; <iop>; store
/// - `Mod rA, rB, rC`    → load, srem, sign-fixup select, store
/// - `Return0`           → ret i64 0
/// - `Return1 rA`        → load regs[A]; ret
/// - `Jmp`               → br bb_<target>
/// - `Lt|Le|Eq rA,rB,k`  → load, icmp, condbr (k flips arms; pc+1 Jmp
///                          provides the false-edge target)
fn compile_compute_chunk(plan: &ChunkPlan) -> Option<(*const u8, EnginePair)> {
    let ctx_box: Box<Context> = Box::new(Context::create());
    // SAFETY: `ctx_box` is heap-allocated and never moved out of the
    // `EnginePair` it lands in via `finalize_module`. The static
    // lifetime is a localised lie — the inkwell `ExecutionEngine`
    // borrows from the context via this `&'static Context`, and the
    // EnginePair's field declaration order (engine first, context
    // second) ensures Rust drops the engine *before* the context, so
    // the borrow stays live for the engine's observable lifetime.
    let ctx_static: &'static Context = unsafe { &*(ctx_box.as_ref() as *const Context) };

    let module = ctx_static.create_module("luna_jit_llvm_compute");
    let builder = ctx_static.create_builder();

    let i64_type = ctx_static.i64_type();
    let regs_ty = i64_type.array_type(plan.num_regs);
    // v2.1 Phase 1K.F.6 — parametric chunks. The fn signature widens
    // from `fn() -> i64` to `fn(i64, …, i64) -> i64` with one i64 per
    // declared positional param, then the entry BB stores each
    // function arg into the matching `regs[i]` slot so the lowerer
    // sees param 0..N-1 as live register sources.
    let param_types: Vec<inkwell::types::BasicMetadataTypeEnum> =
        (0..plan.num_params).map(|_| i64_type.into()).collect();
    let fn_type = i64_type.fn_type(&param_types, false);
    let function = module.add_function("luna_jit_llvm_entry", fn_type, None);

    // v2.1 Phase 1K.F.2 — declare every `luna_jit_*` helper as an
    // external IR function. Used by Op::GetUpval / Op::Call emit
    // below; the dead-locals path skips this step.
    let helpers = declare_jit_helpers(ctx_static, &module);

    // Pre-create one LLVM BB per source BB. The PC-keyed map gives
    // O(1) lookup for branch targets.
    let n = plan.code.len();
    let mut bb_of_pc: Vec<Option<BasicBlock<'static>>> = vec![None; n];
    for (pc, start) in plan.bb_starts.iter().enumerate() {
        if *start && plan.reachable[pc] {
            bb_of_pc[pc] = Some(ctx_static.append_basic_block(function, &format!("bb_{pc}")));
        }
    }
    let entry_bb = bb_of_pc[0]?;
    builder.position_at_end(entry_bb);

    // Allocate the chunk's register file in the entry BB. All other
    // BBs read/write via the same alloca pointer; LLVM mem2reg /
    // SROA will promote scalar slots out of memory at higher opt
    // levels (currently OptimizationLevel::None — promotion lands
    // when 1K.E benches start measuring).
    let regs = builder.build_alloca(regs_ty, "regs").ok()?;

    // v2.1 Phase 1K.F.6 — populate `regs[0..num_params]` from the fn
    // arg list. After this prologue the lowerer sees param 0..N-1 as
    // ordinary live register sources, identical to a chunk that
    // bound them with `LoadI` / `Move`.
    {
        let zero_i64 = i64_type.const_zero();
        for i in 0..plan.num_params {
            let off = i64_type.const_int(i as u64, false);
            let slot = unsafe {
                builder
                    .build_in_bounds_gep(regs_ty, regs, &[zero_i64, off], "param_slot")
                    .ok()?
            };
            let arg = function.get_nth_param(i)?.into_int_value();
            builder.build_store(slot, arg).ok()?;
        }
    }

    let mut emitter = ComputeEmitter {
        ctx: ctx_static,
        builder: &builder,
        function,
        i64_type,
        regs_ty,
        regs,
        helpers: &helpers,
    };

    // Walk PCs; switch BB on bb_starts boundaries; terminators
    // (Return*/Jmp/Lt|Le|Eq) handled here; non-CF ops delegated to
    // `emitter.emit_op`.
    let mut bb_terminated = false;
    let mut current_bb = Some(entry_bb);
    let mut pc = 0usize;
    while pc < n {
        // Skip unreachable ops — they have no BB and would not be
        // valid emit targets.
        if !plan.reachable[pc] {
            pc += 1;
            continue;
        }

        // Entering a new BB? Either we just emitted a terminator (in
        // which case we MUST switch) or the prev BB fell through to
        // a BB-start (insert an unconditional br).
        if plan.bb_starts[pc] && current_bb != bb_of_pc[pc] {
            let next_bb = bb_of_pc[pc]?;
            if !bb_terminated {
                builder.build_unconditional_branch(next_bb).ok()?;
            }
            builder.position_at_end(next_bb);
            current_bb = Some(next_bb);
            bb_terminated = false;
        }

        // Consumed Jmp (folded into a preceding Lt|Le|Eq condbr) —
        // skip emit entirely. The condbr already wrote the
        // terminator for this BB.
        if plan.consumed_jmp[pc] {
            pc += 1;
            continue;
        }

        let ins = plan.code[pc];
        match ins.op() {
            Op::Return0 => {
                let zero = i64_type.const_zero();
                builder.build_return(Some(&zero)).ok()?;
                bb_terminated = true;
            }
            Op::Return1 => {
                let v = emitter.load_reg(ins.a(), "ret_val")?;
                builder.build_return(Some(&v)).ok()?;
                bb_terminated = true;
            }
            Op::Jmp => {
                let tgt = jmp_target(pc, ins);
                let tgt_bb = bb_of_pc.get(tgt).copied().flatten()?;
                builder.build_unconditional_branch(tgt_bb).ok()?;
                bb_terminated = true;
            }
            Op::Lt | Op::Le | Op::Eq => {
                // Lua predicate semantics:
                //   if ((R[A] <op> R[B]) ~= k) then pc++
                // i.e. SKIP the next Jmp when the comparison's truth
                // value differs from k. Mapped to a single LLVM
                // condbr by picking the (then/else) branches per k:
                //   k = true  → then = jmp_bb, else = fall_bb
                //   k = false → then = fall_bb, else = jmp_bb
                // because:
                //   cmp == k  ↔ DON'T skip ↔ take the Jmp
                //   cmp != k  ↔ SKIP       ↔ take the fall-through
                let pred = match ins.op() {
                    Op::Lt => inkwell::IntPredicate::SLT,
                    Op::Le => inkwell::IntPredicate::SLE,
                    Op::Eq => inkwell::IntPredicate::EQ,
                    _ => unreachable!(),
                };
                let lhs = emitter.load_reg(ins.a(), "cmp_lhs")?;
                let rhs = emitter.load_reg(ins.b(), "cmp_rhs")?;
                let cmp = builder.build_int_compare(pred, lhs, rhs, "cmp_res").ok()?;
                let jmp_ins = plan.code.get(pc + 1)?;
                let jmp_pc = pc + 1;
                let jmp_target_pc = jmp_target(jmp_pc, *jmp_ins);
                let fall_pc = pc + 2;
                let fall_bb = bb_of_pc.get(fall_pc).copied().flatten()?;
                let jmp_bb = bb_of_pc.get(jmp_target_pc).copied().flatten()?;
                let (then_bb, else_bb) = if ins.k() {
                    (jmp_bb, fall_bb)
                } else {
                    (fall_bb, jmp_bb)
                };
                builder
                    .build_conditional_branch(cmp, then_bb, else_bb)
                    .ok()?;
                bb_terminated = true;
            }
            Op::GetUpval => {
                let dst = ins.a();
                if plan.is_upval_value_read[pc] {
                    // 1K.F.4 ValueRead — call luna_jit_upval_get(b);
                    // store result into regs[A].
                    let helper = emitter.helpers.get("luna_jit_upval_get").copied()?;
                    let idx_arg = i64_type.const_int(ins.b() as u64, false);
                    let call_inst = builder
                        .build_call(helper, &[idx_arg.into()], "upv_val")
                        .ok()?;
                    let v = match call_inst.try_as_basic_value() {
                        inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
                        inkwell::values::ValueKind::Instruction(_) => return None,
                    };
                    let slot = emitter.reg_slot_ptr(dst, "upv_dst")?;
                    builder.build_store(slot, v).ok()?;
                } else {
                    // 1K.F.4 SelfMarker — destination register is never
                    // read as a value (its only consumer is a
                    // subsequent Op::Call which lowers as a direct
                    // self-call). Store a placeholder 0 so the alloca
                    // slot is initialised in case any path leaks a
                    // read; mirrors Cranelift's `aligned_def(..., 0)`.
                    let zero = i64_type.const_zero();
                    let slot = emitter.reg_slot_ptr(dst, "upv_self_marker")?;
                    builder.build_store(slot, zero).ok()?;
                }
            }
            Op::Call => {
                // 1K.F.3 — only the self-recursive shape lands here
                // (whitelist + tracker gated). Emit a direct
                // `build_call` against the current entry function and
                // store the i64 result into regs[A].
                debug_assert!(
                    plan.self_call_pcs[pc],
                    "scanner accepts only self-recursive Op::Call PCs"
                );
                let a = ins.a();
                let nargs = ins.b().saturating_sub(1);
                let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'static>> =
                    Vec::with_capacity(nargs as usize);
                for off in 1..=nargs {
                    let v = emitter.load_reg(a + off, "call_arg")?;
                    arg_vals.push(v.into());
                }
                let call_inst = builder
                    .build_call(emitter.function, &arg_vals, "self_call")
                    .ok()?;
                let v = match call_inst.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
                    inkwell::values::ValueKind::Instruction(_) => return None,
                };
                let slot = emitter.reg_slot_ptr(a, "call_dst")?;
                builder.build_store(slot, v).ok()?;
            }
            Op::TailCall => {
                // 1K.G — self-recursive tail call (tracker gated).
                // Semantics: call function(R[A+1..A+nargs]) and return
                // its result directly to our caller. Emits as
                // build_call + ret, equivalent to Op::Call + Return1.
                debug_assert!(
                    plan.tail_call_pcs[pc],
                    "scanner accepts only self-recursive Op::TailCall PCs"
                );
                let a = ins.a();
                let nargs = ins.b().saturating_sub(1);
                let mut arg_vals: Vec<inkwell::values::BasicMetadataValueEnum<'static>> =
                    Vec::with_capacity(nargs as usize);
                for off in 1..=nargs {
                    let v = emitter.load_reg(a + off, "tail_arg")?;
                    arg_vals.push(v.into());
                }
                let call_inst = builder
                    .build_call(emitter.function, &arg_vals, "tail_call")
                    .ok()?;
                let v = match call_inst.try_as_basic_value() {
                    inkwell::values::ValueKind::Basic(bv) => bv.into_int_value(),
                    inkwell::values::ValueKind::Instruction(_) => return None,
                };
                // Return the tail call result directly — no regs[A] store.
                builder.build_return(Some(&v)).ok()?;
                bb_terminated = true;
            }
            _ => {
                emitter.emit_op(ins)?;
            }
        }
        pc += 1;
    }

    // Sanity: a well-formed chunk's last reachable BB ends with a
    // terminator (every parser-emitted chunk ends with `Return*`).
    // If we somehow exited the loop with an unterminated BB, the
    // resulting LLVM IR would be malformed — bail rather than emit it.
    if !bb_terminated {
        return None;
    }

    finalize_module(ctx_box, module, Some(&helpers))
}

/// Per-op emit context for the compute path. Holds the LLVM types
/// and the entry block's register-file alloca; each `emit_op` call
/// appends the op's IR sequence at the builder's current position.
///
/// `emit_op` handles non-control-flow ops (LoadI / LoadNil / Move /
/// Add / Sub / Mul / Mod). Control-flow ops (`Return0|Return1|Jmp|
/// Lt|Le|Eq`) are handled in [`compile_compute_chunk`] directly so
/// the outer loop can switch BBs around the emitted terminator.
struct ComputeEmitter<'ctx, 'a> {
    #[allow(dead_code)] // Held for future per-op IR (intrinsics / strings).
    ctx: &'ctx Context,
    builder: &'a inkwell::builder::Builder<'ctx>,
    /// v2.1 Phase 1K.F.3 — held so the Op::Call lowerer can emit a
    /// `build_call` against the current entry `FunctionValue` for
    /// self-recursive shapes. Mirrors Cranelift's
    /// `module.declare_func_in_func(fn_id, bcx.func)` pattern.
    function: FunctionValue<'ctx>,
    i64_type: inkwell::types::IntType<'ctx>,
    regs_ty: inkwell::types::ArrayType<'ctx>,
    regs: PointerValue<'ctx>,
    /// v2.1 Phase 1K.F.4 — `luna_jit_*` helper declarations.
    /// Op::GetUpval (ValueRead role) reaches for
    /// `helpers["luna_jit_upval_get"]`; future ops widen the call sites
    /// without per-op registration boilerplate.
    helpers: &'a HashMap<&'static str, FunctionValue<'ctx>>,
}

impl<'ctx, 'a> ComputeEmitter<'ctx, 'a> {
    /// GEP the `idx`-th register slot inside the alloca.
    fn reg_slot_ptr(&self, idx: u32, name: &str) -> Option<PointerValue<'ctx>> {
        let zero = self.i64_type.const_zero();
        let off = self.i64_type.const_int(idx as u64, false);
        // SAFETY: `[0, idx]` indexes into a `[regs_ty x i64]` alloca
        // sized `plan.num_regs`. `ChunkPlan::from_proto` checked that
        // the largest A in the recognised ops fits; the per-op
        // emitters below all clamp to the alloca's bounds.
        unsafe {
            self.builder
                .build_in_bounds_gep(self.regs_ty, self.regs, &[zero, off], name)
                .ok()
        }
    }

    /// Store an i64 immediate into `regs[idx]`.
    fn store_imm(&self, idx: u32, imm: i64) -> Option<()> {
        let slot = self.reg_slot_ptr(idx, "imm_slot")?;
        let val = self.i64_type.const_int(imm as u64, true);
        self.builder.build_store(slot, val).ok()?;
        Some(())
    }

    /// Load `regs[idx]` as an i64.
    fn load_reg(&self, idx: u32, name: &str) -> Option<inkwell::values::IntValue<'ctx>> {
        let slot = self.reg_slot_ptr(idx, name)?;
        let v = self.builder.build_load(self.i64_type, slot, name).ok()?;
        Some(v.into_int_value())
    }

    /// Phase 1K.E.3 / 1K.E.4 — shared emit for `R[A] = R[B] <op> R[C]`
    /// where `<op>` is a single-instruction LLVM int binop (Add / Sub
    /// / Mul / ...). Loads `b`/`c`, applies `op_fn`, stores into `a`.
    fn emit_int_binop<F>(&self, ins: Inst, label: &str, op_fn: F) -> Option<()>
    where
        F: Fn(
            &inkwell::builder::Builder<'ctx>,
            inkwell::values::IntValue<'ctx>,
            inkwell::values::IntValue<'ctx>,
            &str,
        ) -> Option<inkwell::values::IntValue<'ctx>>,
    {
        let a = ins.a();
        let b = ins.b();
        let c = ins.c();
        let lhs = self.load_reg(b, &format!("{label}_lhs"))?;
        let rhs = self.load_reg(c, &format!("{label}_rhs"))?;
        let result = op_fn(self.builder, lhs, rhs, &format!("{label}_res"))?;
        let dst = self.reg_slot_ptr(a, &format!("{label}_dst"))?;
        self.builder.build_store(dst, result).ok()?;
        Some(())
    }

    fn emit_op(&mut self, ins: Inst) -> Option<()> {
        match ins.op() {
            Op::LoadI => {
                let a = ins.a();
                let sbx = ins.sbx() as i64;
                self.store_imm(a, sbx)?;
                Some(())
            }
            Op::LoadNil => {
                // `R[A..=A+B] = nil`. The compute path treats nil as
                // the i64 bit-pattern 0; that's a sound choice for
                // chunks that return ints (Return1 reads i64 directly)
                // because no recognised op observes nil as a distinct
                // tag. When 1K.E later adds bool/value-tagged ops this
                // will need to switch to tagged bit patterns.
                let a = ins.a();
                let b = ins.b();
                for off in 0..=b {
                    self.store_imm(a + off, 0)?;
                }
                Some(())
            }
            Op::Move => {
                let a = ins.a();
                let b = ins.b();
                let v = self.load_reg(b, "move_src")?;
                let dst = self.reg_slot_ptr(a, "move_dst")?;
                self.builder.build_store(dst, v).ok()?;
                Some(())
            }
            Op::Add => {
                // Phase 1K.E.3 — int add `R[A] = R[B] + R[C]`. Pure
                // signed-i64 add; no overflow check (the int-chunk
                // ABI silently wraps, matching Lua 5.4's integer
                // arithmetic semantics for the `+` operator on two
                // ints — `math.maxinteger + 1 == math.mininteger`).
                //
                // No type-tag inspection: the compute whitelist
                // currently has no op that produces a non-int value
                // into a reg (LoadNil → 0, LoadI/Move → ints, this
                // Add → int). When 1K.E.7 adds LoadFalse/LoadTrue
                // (which produce bool bit patterns) or LoadK(Float)
                // is whitelisted, this arm will need to refuse
                // (or wrap with) cross-type operands.
                self.emit_int_binop(ins, "add", |b, l, r, n| b.build_int_add(l, r, n).ok())
            }
            Op::Sub => {
                // Phase 1K.E.4 — int sub `R[A] = R[B] - R[C]`. Same
                // wrapping i64 semantics as Add.
                self.emit_int_binop(ins, "sub", |b, l, r, n| b.build_int_sub(l, r, n).ok())
            }
            Op::Mul => {
                // Phase 1K.E.4 — int mul `R[A] = R[B] * R[C]`. Same
                // wrapping i64 semantics as Add.
                self.emit_int_binop(ins, "mul", |b, l, r, n| b.build_int_mul(l, r, n).ok())
            }
            Op::Mod => {
                // Phase 1K.E.4 — Lua-semantic int mod.
                //
                // Lua 5.4 / 5.5 `%` for two ints:
                //     R[A] = R[B] - floor(R[B] / R[C]) * R[C]
                // which differs from C's `%` (truncating remainder)
                // when the operand signs differ. Examples:
                //
                //   |  a |  b |  a % b (Lua) |  a srem b (C) |
                //   |----|----|--------------|---------------|
                //   |  7 |  3 |       1      |        1      |
                //   | -7 |  3 |       2      |       -1      |
                //   |  7 | -3 |      -2      |        1      |
                //   | -7 | -3 |      -1      |       -1      |
                //
                // LLVM's `srem` matches the C semantics, so we adjust:
                //     r = srem(a, b)
                //     r != 0  AND  (r ^ b) < 0   ⇒  r += b
                // (the "(r ^ b) < 0" test asks "do r and b have
                // different signs?"; combined with r != 0 it catches
                // exactly the rows above where Lua and C disagree.)
                //
                // Branch-free via `select`. Division-by-zero is the
                // interpreter's job (it raises "attempt to perform
                // 'n%%0'"); a Mod chunk that statically uses zero as
                // R[C] still wouldn't reach here through normal
                // parser-emitted bytecode, so we accept LLVM's UB on
                // `srem x, 0` rather than emit a runtime check.
                let a = ins.a();
                let b = ins.b();
                let c = ins.c();
                let lhs = self.load_reg(b, "mod_lhs")?;
                let rhs = self.load_reg(c, "mod_rhs")?;
                let raw = self
                    .builder
                    .build_int_signed_rem(lhs, rhs, "mod_srem")
                    .ok()?;
                let zero = self.i64_type.const_zero();
                let nonzero = self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::NE, raw, zero, "mod_raw_nonzero")
                    .ok()?;
                // Sign-differ test: (raw XOR rhs) < 0 ↔ MSBs differ.
                let xor = self.builder.build_xor(raw, rhs, "mod_sign_xor").ok()?;
                let sign_differ = self
                    .builder
                    .build_int_compare(inkwell::IntPredicate::SLT, xor, zero, "mod_sign_differ")
                    .ok()?;
                let need_fix = self
                    .builder
                    .build_and(nonzero, sign_differ, "mod_need_fix")
                    .ok()?;
                let fixed = self.builder.build_int_add(raw, rhs, "mod_fixed").ok()?;
                let result = self
                    .builder
                    .build_select(need_fix, fixed, raw, "mod_result")
                    .ok()?
                    .into_int_value();
                let dst = self.reg_slot_ptr(a, "mod_dst")?;
                self.builder.build_store(dst, result).ok()?;
                Some(())
            }
            _ => {
                // Whitelist guarded this in `ChunkPlan::from_proto`;
                // control-flow ops (Return0/Return1/Jmp/Lt/Le/Eq)
                // are handled in `compile_compute_chunk`'s outer
                // loop, not here. Any other op slipping through =
                // someone added it to the whitelist without an
                // emit arm; bail rather than emit junk.
                None
            }
        }
    }
}

/// Shared module-finalisation tail. JIT-compiles `module` under the
/// (heap-pinned) `ctx_box`, resolves the entry symbol, and bundles
/// both into an [`EnginePair`] for storage.
///
/// `helpers` is `Some(map)` for paths that may invoke `luna_jit_*`
/// helpers (the Phase 1K.F compute path) — every entry gets bound to
/// its real Rust function address via `add_global_mapping`. The
/// Phase 1K.D dead-locals path passes `None` because its IR makes no
/// helper calls and skipping the map keeps that fast-path tight.
/// v2.1 Phase 1K.G — also used by the trace JIT (`trace.rs`).
pub(crate) fn finalize_module<'ctx>(
    ctx_box: Box<Context>,
    module: Module<'ctx>,
    helpers: Option<&HashMap<&'static str, FunctionValue<'ctx>>>,
) -> Option<(*const u8, EnginePair)> {
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::None)
        .ok()?;
    if let Some(map) = helpers {
        bind_helper_symbols(&engine, map);
    }
    let entry_ptr = engine.get_function_address("luna_jit_llvm_entry").ok()? as *const u8;
    // SAFETY: `EnginePair` holds the engine as
    // `ExecutionEngine<'static>` — see the constructor of
    // `compile_compute_chunk` for the lifetime upgrade discussion.
    // The transmute below relabels `EE<'ctx>` to `EE<'static>`; the
    // `'ctx` borrow stays live for the engine's observable lifetime
    // because `ctx_box` is moved into the pair on the same line and
    // pinned by struct field-order drop.
    let engine_static: inkwell::execution_engine::ExecutionEngine<'static> =
        unsafe { std::mem::transmute(engine) };
    let pair = EnginePair {
        engine: engine_static,
        context: ctx_box,
    };
    Some((entry_ptr, pair))
}
