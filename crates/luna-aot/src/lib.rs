#![warn(missing_docs)]
//! luna-aot — ahead-of-time compiler from Lua source to a
//! self-contained native binary.
//!
//! # v1.3 ship scope (THIS SESSION — scaffold + bytecode-embed only)
//!
//! What runs end-to-end **today**:
//!
//! 1. CLI [`cli::run`] / library [`embed::embed_bytecode`] take a `.lua`
//!    source file.
//! 2. luna-core's frontend parses + compiles it to bytecode (a
//!    [`luna_core::runtime::Proto`] tree).
//! 3. The bytecode is dumped via [`luna_core::vm::dump`] (luna's own
//!    body format — `"\x1bLua" + version-byte` header + the
//!    `"\x00LunaV1\x00"` sentinel + luna body).
//! 4. The dump bytes are written into a `.luna.bytecode` section of an
//!    ELF / Mach-O / PE object file via [`object::write::Object`],
//!    bracketed by two **public** symbols
//!    `__luna_bytecode_start` and `__luna_bytecode_end` that the
//!    runtime stub (or a custom host binary) `extern "C"`s.
//! 5. The CLI invokes system `cc` to link the bytecode object + a
//!    minimal C entry point into a final executable. The default
//!    scaffold entry just prints the embedded bytecode length to
//!    `stderr` and exits — proving the section is reachable end-to-end.
//!
//! What is **deferred to follow-up sessions** within the v1.3 mega
//! sprint (per `.dev/rfcs/v1.3-audit-luna-aot.md` Stage 3-6):
//!
//! - **Wiring the [`runtime_stub`] Rust module into the linked binary**
//!   so the embedded bytecode is `undump`-ed into a [`luna_core::vm::Vm`]
//!   and executed at process start (interp-only). This needs either
//!   (a) `luna-core` exposed as a `staticlib` for the target triple,
//!   or (b) a tempdir-cargo shell-out that builds a tiny crate that
//!   `include_bytes!`s the dump and depends on `luna-core`. The
//!   runtime-stub source compiles cleanly today; it just isn't wired
//!   to the link step yet.
//! - **Cranelift trace codegen via `cranelift-object::ObjectModule`**
//!   (the bulk of the ~70-day audit estimate — the lowerer refactor
//!   that genericises the JIT backend over `cranelift_module::Module`).
//! - **Cross-compile** via `--target <triple>`. The flag is present in
//!   the CLI today but only the host triple is wired end-to-end.
//! - **Smoke tests** that run the produced binary and compare stdout
//!   with `luna`.
//!
//! # Why a separate crate
//!
//! See the audit's "Why a separate crate vs `--features aot`" section.
//! Summary: this crate pulls `object` + `clap` and (in a follow-up
//! session) all of cranelift; embedders who only want the runtime
//! JIT keep using `luna-jit`, and embedders who only want pure interp
//! keep using `luna-core` — both stay free of `luna-aot`'s build-time
//! dep tree.
//!
//! **`luna-core` 0-third-party-dep contract is unaffected.** This
//! crate depends on `luna-core` but `luna-core` does not depend on
//! anything here. The CI gate
//! (`cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"`)
//! continues to report `1`.

/// Symbol name marking the start of the embedded bytecode section.
/// External (C-ABI) symbol; the runtime stub declares it as
/// `extern "C" { static __luna_bytecode_start: u8; }`.
pub const BYTECODE_START_SYMBOL: &str = "__luna_bytecode_start";

/// Symbol name marking the end of the embedded bytecode section.
/// External (C-ABI) symbol; the runtime stub declares it as
/// `extern "C" { static __luna_bytecode_end: u8; }` and computes the
/// length as `(&end as *const u8).offset_from(&start as *const u8)`.
pub const BYTECODE_END_SYMBOL: &str = "__luna_bytecode_end";

/// Object-file section name for the embedded bytecode.
/// ELF/Mach-O conventions: a leading dot is the standard for non-loader-
/// special sections; we pick `.luna.bytecode` so `objdump -s -j
/// .luna.bytecode <out>` displays the dump bytes for inspection.
pub const BYTECODE_SECTION_NAME: &str = ".luna.bytecode";

pub mod cli;
pub mod embed;
pub mod runtime_stub;
