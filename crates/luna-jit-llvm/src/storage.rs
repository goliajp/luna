//! v2.1 Phase 1K.D.8 — per-`Vm` LLVM JIT storage cache.
//!
//! Mirrors the role of `luna_jit::jit_backend::storage::CraneliftJitStorage`:
//! the cache maps a proto's stable bytecode key to its previously
//! JIT-compiled entry pointer + metadata, and an owning vector keeps
//! the `inkwell::ExecutionEngine` instances alive (one per compile)
//! so the JIT mcode mmap stays callable for the Vm's lifetime.
//!
//! ## Lifetime path (Risk #1 in 1K.C audit § 6 — provisional)
//!
//! `inkwell::Context` borrows from itself (`<'ctx>`), so
//! `ExecutionEngine<'ctx>`, `Module<'ctx>`, and `Builder<'ctx>` all
//! pick up that lifetime. luna's storage needs to own the
//! `Context` AND a growing collection of `ExecutionEngine`s — three
//! options per the audit:
//!   (a) `ouroboros` self-referential macro (new dep, unsafe internally);
//!   (b) hand-rolled `transmute<EE<'_>, EE<'static>>` with strict drop
//!       order discipline;
//!   (c) per-compile throwaway `Context` (slower but trivially correct).
//!
//! Phase 1K.D.8 picks **option (b) light**: each compile gets its
//! own freshly-`Box::leak`-ed `Context` (so `Context` is `'static`),
//! then its `ExecutionEngine` naturally lives `'static` too and
//! lands in the cache `Vec` as a `Box<ExecutionEngine<'static>>`.
//! Memory grows linearly with first-compile count (no growth on
//! cache hit). Phase 1K.E will revisit when the Vm-context unit
//! economics need tuning — likely moving to option (b) proper with
//! one shared Context per Vm if the per-compile init cost becomes
//! measurable in benches.
//!
//! ## Cache invalidation
//!
//! None. Compiled entries live for the Vm's lifetime; dropping the
//! `LlvmJitStorage` drops every `ExecutionEngine`, which unmaps
//! the JIT mcode pages. Tests can call [`LlvmJitStorage::clear`]
//! to force a fresh compile between cases.

use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use luna_core::jit::{CompileResult, JitStorage};
use std::collections::HashMap;

/// Cached compile result for a single Proto key. `entry` is the
/// JIT-compiled entry pointer; the surrounding metadata mirrors the
/// `CompileResult::Compiled` payload.
#[derive(Clone, Copy)]
pub(crate) struct CachedEntry {
    pub entry: *const u8,
    pub num_args: u8,
    pub returns_one: bool,
    pub arg_float_mask: u8,
    pub arg_table_mask: u8,
    pub ret_is_float: bool,
    pub ret_is_table: bool,
}

impl CachedEntry {
    pub(crate) fn to_compile_result(self) -> CompileResult {
        CompileResult::Compiled {
            entry: self.entry,
            num_args: self.num_args,
            returns_one: self.returns_one,
            arg_float_mask: self.arg_float_mask,
            arg_table_mask: self.arg_table_mask,
            ret_is_float: self.ret_is_float,
            ret_is_table: self.ret_is_table,
        }
    }
}

/// LLVM-side per-`Vm` JIT storage cache.
///
/// Two collections:
/// - [`Self::cache`] — proto-key → cached entry. Lookups O(1).
/// - [`Self::engines`] — owning vector of leaked
///   `Box<ExecutionEngine<'static>>` so the JIT mmap stays alive
///   for the Vm's lifetime.
///
/// The cache key (`u64`) is computed by the codegen module from the
/// proto's bytecode + constants (mirrors the Cranelift backend's
/// `proto_cache_key`); the storage stays codegen-key-agnostic.
#[derive(Default)]
pub struct LlvmJitStorage {
    pub(crate) cache: HashMap<u64, CachedEntry>,
    /// Owning rooted pairs of `(Context, ExecutionEngine)` keeping
    /// the JIT mmap alive. Stored as `*mut ()` because the underlying
    /// values are heterogeneous-lifetime inkwell types that don't
    /// have a stable concrete type without the `'ctx` parameter.
    /// Each entry is a `Box::into_raw` of an `EnginePair` allocated
    /// on the heap; `Drop` walks the vec and reclaims them.
    pub(crate) engines: Vec<*mut EnginePair>,
}

/// Heap-allocated `(Context, ExecutionEngine)` pair. Stored behind a
/// raw pointer in [`LlvmJitStorage::engines`] because Rust's
/// borrow checker can't express the self-referential
/// `EE<'self::ctx>` relationship without `ouroboros`. The field
/// order matters: `engine` must be dropped before `context` — Rust
/// drops struct fields in declaration order, so engine first is the
/// safe layout.
pub(crate) struct EnginePair {
    // SAFETY: the `'static` here is a lie that the constructor
    // upholds by allocating `Context` on the heap before producing
    // the `ExecutionEngine` and never moving / dropping `Context`
    // out of the pair. `context` is kept after `engine` in field
    // order so it outlives the engine in drop order. The fields
    // are only read structurally by `Drop`; neither is exposed to
    // the trait surface.
    #[allow(dead_code)]
    pub engine: ExecutionEngine<'static>,
    #[allow(dead_code)]
    pub context: Box<Context>,
}

impl LlvmJitStorage {
    /// Drop every cached entry + ExecutionEngine. Tests use this to
    /// force a fresh compile between cases.
    pub fn clear(&mut self) {
        self.cache.clear();
        for raw in self.engines.drain(..) {
            // SAFETY: each pointer in `engines` originated from
            // `Box::into_raw` in `LlvmJitStorage::insert` below; we
            // reclaim it exactly once here.
            drop(unsafe { Box::from_raw(raw) });
        }
    }

    /// Number of compiled entries currently cached. Used by tests
    /// to verify a second compile of the same Proto hits the cache
    /// rather than reusing storage (= the count goes up by 1, not
    /// 2, on a hit).
    pub fn cache_entry_count(&self) -> usize {
        self.cache.len()
    }

    /// Park a freshly-compiled (Context, EE) pair on the cache. The
    /// `EnginePair` is heap-allocated and the pointer kept in
    /// [`Self::engines`]; on drop / `clear` the box is reclaimed.
    pub(crate) fn insert(&mut self, key: u64, pair: EnginePair, entry: CachedEntry) {
        let boxed = Box::into_raw(Box::new(pair));
        self.engines.push(boxed);
        self.cache.insert(key, entry);
    }

    /// v2.1 Phase 1K.G — park a trace `EnginePair` so the JIT mmap
    /// stays alive for the Vm's lifetime. Unlike [`Self::insert`] there
    /// is no cache-key association — the `TraceFn` pointer embedded in
    /// `CompiledTrace::entry` is the caller's handle; storage just owns
    /// the lifetime.
    pub(crate) fn park_engine(&mut self, pair: EnginePair) {
        let boxed = Box::into_raw(Box::new(pair));
        self.engines.push(boxed);
    }
}

impl Drop for LlvmJitStorage {
    fn drop(&mut self) {
        self.clear();
    }
}

impl JitStorage for LlvmJitStorage {
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
