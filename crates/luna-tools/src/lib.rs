#![warn(missing_docs)]
//! luna-tools — developer-facing inspection + introspection CLIs.
//!
//! # v2.0 Track TL — Phase 1 + Phase 2 status
//!
//! Ships five binaries under one workspace member:
//!
//! | Binary | Phase | Status |
//! | --- | --- | --- |
//! | `luna-bin-inspect` | 1 | Real — walks an AOT-produced binary's `.luna.bytecode` / `luna_trace_meta` / `luna_inline_chnx` sections and reports a section table + trace index counts. |
//! | `luna-heap-dump` | 1 | Real — runs a `.lua` script in a [`luna_jit::Vm`], then prints a per-type snapshot (object count + approximate bytes) using the pure-read accessors in [`luna_jit::inspect`]. |
//! | `luna-trace-inspect` | 2 | Real — runs a `.lua` script and dumps the resulting [`luna_jit::inspect::JitStateSnapshot`] (counters + active-trace head_pc / ops_len). `--show ir` + `--show mcode` are reserved CLI surface; they exit non-zero today with a pointer to the Track R IR stabilisation / capstone-feature tracking docs per `.dev/rfcs/v2.0-audit-tl.md` § R1. |
//! | `luna-profile` | 2 | Real — Count-hook sampling profiler. Text top-N + folded-stack output for `inferno-flamegraph`. `--format pprof` is reserved for the `flame-graph` feature opt-in per audit R2. |
//! | `luna-repl-polish` | 2 | Stub — `unimplemented!`; rustyline `=14.x` pin lands with the impl per audit R3. |
//!
//! The remaining stub (`luna-repl-polish`) is not a silent skip: it
//! calls [`unimplemented!`] with a pointer to the tracking document
//! so `cargo install luna-tools` followed by `luna-repl-polish` exits
//! non-zero with a clear message instead of pretending to work.
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
