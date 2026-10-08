#![warn(missing_docs)]
//! luna-aot — ahead-of-time compiler from Lua source to a
//! self-contained native binary.
//!
//! # Scaffold pipeline (bytecode embed)
//!
//!
//! 1. CLI [`cli::run`] / library [`embed::embed_bytecode`] take a `.lua`
//!    source file.
//! 2. luna-core's frontend parses + compiles it to bytecode (a
//!    [`luna_core::runtime::Proto`] tree).
//! 3. The bytecode is dumped via [`luna_core::vm::dump`] (luna's own
//!    body format — `"\x1bLua" + version-byte` header + the
//!    `"\x00LunaV4\x00"` sentinel + luna body).
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
//! # Why a separate crate
//!
//! This crate pulls `object` + `clap` and all of cranelift;
//! embedders who only want the runtime JIT keep using `luna-jit`, and
//! embedders who only want pure interp keep using `luna-core` — both stay free of `luna-aot`'s build-time
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
// `runtime_stub` declares `extern "C" { static __luna_bytecode_start: u8 }`
// + `__luna_bytecode_end`. These symbols are linker-provided by the
// bytecode `.o` that `embed::embed_bytecode` writes; they exist only in
// the AOT-produced binary's link set, not in the luna-aot crate's own
// builds (rlib / test / doctest).
//
// Most linkers (ld / lld / Mach-O) tolerate unresolved `extern`
// references unless something actually calls them. MSVC's `link.exe`
// is stricter — even unused references trigger LNK2019. So the module
// is gated off when building the luna-aot crate itself under MSVC;
// the user-side Cargo bootstrap recipe (see `runtime_stub::aot_main`
// rustdoc § Wiring) re-enables it by setting
// `RUSTFLAGS=--cfg luna_aot_runtime_stub` when compiling against the
// real bytecode object link set. Tier-1 deploys via the AOT pipeline's
// own `cc` invocation (`embed::compile_and_link`) use the C entry +
// `luna_aot_run` from `luna-runtime-helpers`, not this Rust stub.
#[cfg(any(not(target_env = "msvc"), luna_aot_runtime_stub))]
pub mod runtime_stub;
