#![warn(missing_docs)]
//! luna-tools — developer-facing inspection + introspection CLIs.
//!
//! # v2.0 Track TL Phase 1 (THIS COMMIT) — scaffold + 2 tools real
//!
//! Ships five binaries under one workspace member:
//!
//! | Binary | Phase | Status |
//! | --- | --- | --- |
//! | `luna-bin-inspect` | 1 | Real — walks an AOT-produced binary's `.luna.bytecode` / `luna_trace_meta` / `luna_inline_chnx` sections and reports a section table + trace index counts. |
//! | `luna-heap-dump` | 1 | Real — runs a `.lua` script in a [`luna_jit::Vm`], then prints a per-type snapshot (object count + approximate bytes) using the new pure-read accessors in [`luna_jit::inspect`]. |
//! | `luna-profile` | 2 | Stub — `unimplemented!` body; ships in Track TL Phase 2 (needs `--features flame-graph` for `inferno` + `pprof`). |
//! | `luna-trace-inspect` | 2 | Stub — `unimplemented!`; depends on Track R IR shape stabilising first per `.dev/rfcs/v2.0-audit-tl.md` § R1. |
//! | `luna-repl-polish` | 2 | Stub — `unimplemented!`; rustyline `=14.x` pin lands with the impl per audit R3. |
//!
//! Stubs are not silent skips: they call [`unimplemented!`] with a
//! pointer to the tracking document so `cargo install luna-tools`
//! followed by `luna-profile --foo` exits non-zero with a clear
//! message instead of pretending to work.
//!
//! # Why one crate for all five binaries
//!
//! - One Cargo install pins all tool binaries on the user's `$PATH`
//!   so muscle-memory survives future tool additions.
//! - Shared JSON output schema (this lib crate's [`schema`] module)
//!   stays single-sourced — `luna-heap-dump`'s `--out json` and
//!   `luna-bin-inspect`'s `--out json` agree on field shapes for
//!   downstream diffing tools (the eventual `luna-heap-diff` will
//!   parse both sides via [`schema::HeapSnapshot`]).
//! - Heavyweight deps (`inferno`, `pprof`, `capstone`) sit behind
//!   `[features]` gates so embedders who only need bin-inspect /
//!   heap-dump don't pay the supply-chain price.
//!
//! # luna-core 0-dep contract — unaffected
//!
//! luna-tools depends on `luna-jit` (which depends on `luna-core`).
//! `luna-core` itself adds no third-party deps via this crate; the
//! CI gate
//! (`cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"`)
//! continues to report `1`.

pub mod schema;
