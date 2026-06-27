//! v2.1 Phase 1K.D — LLVM 18 + inkwell 0.9 alternative JIT backend
//! for luna.
//!
//! ## Status
//!
//! This crate is the **framework scaffold** — Phase 1K.D.2 lands the
//! crate skeleton, Phase 1K.D.5 wires the trait stubs, Phase 1K.D.6+
//! lights up actual ops one at a time. Most trait methods currently
//! return `CompileResult::Skipped` / `None` so the dispatcher falls
//! through to the interpreter without changing behaviour.
//!
//! ## How luna selects this backend
//!
//! 1. `luna-jit` is built with `--features llvm-jit` (default OFF;
//!    keeps Cranelift as the default and avoids dragging LLVM into
//!    the standard install).
//! 2. At runtime the `LUNA_JIT_BACKEND=llvm` env var flips
//!    `luna_jit::install_default_jit` from `CraneliftBackend` to
//!    `LlvmBackend`.
//!
//! See `.dev/rfcs/v2.1-phase-1k-c-trait-audit.md` § 4.3 for the
//! end-to-end selection design.
//!
//! ## Why a separate crate (vs. a luna-jit submodule)
//!
//! The 1K.C audit (§ 3.5) requires the two backends to ship in
//! independent crates so:
//! - Cranelift is not pulled into the LLVM-only install path.
//! - LLVM is not pulled into the default Cranelift install path
//!   (LLVM adds ~1 GB of system deps on the dev host).
//! - Both backends register the *same* `luna_jit_*` symbol set via
//!   the shared `luna-jit-helpers` crate (single-source-of-truth
//!   for helper definitions).

/// v2.1 Phase 1K.D.2 — LLVM-backed JIT backend zero-sized type. Will
/// implement `luna_core::jit::IntChunkCompiler` + `TraceCompiler` +
/// `JitStorage` in Phase 1K.D.5; until then this is a marker struct
/// so the workspace builds and the `LUNA_JIT_BACKEND=llvm` selector
/// can wire through.
#[derive(Default, Clone, Copy)]
pub struct LlvmBackend;
