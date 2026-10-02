#![warn(missing_docs)]
//! luna-tools — developer-facing inspection + introspection CLIs.
//!
//! # Status
//!
//! Ships five binaries under one workspace member:
//!
//! | Binary | Status |
//! | --- | --- |
//! | `luna-bin-inspect` | Real — walks an AOT-produced binary's `.luna.bytecode` / `luna_trace_meta` / `luna_inline_chnx` sections and reports a section table + trace index counts. |
//! | `luna-heap-dump` | Real — runs a `.lua` script in a [`luna_jit::Vm`], then prints a per-type snapshot (object count + approximate bytes) using the pure-read accessors in [`luna_jit::inspect`]. |
//! | `luna-trace-inspect` | Real — runs a `.lua` script and dumps the resulting [`luna_jit::inspect::JitStateSnapshot`] (counters + active-trace head_pc / ops_len). `--show ir` + `--show mcode` are reserved CLI surface and exit non-zero. |
//! | `luna-profile` | Real — Count-hook sampling profiler. Text top-N + folded-stack output for `inferno-flamegraph`. `--format pprof` is reserved for the `flame-graph` feature opt-in. |
//! | `luna-soak` | Real — long-running workload runner sampling RSS and Vm memory. |
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
//! - Heavyweight deps (`inferno`, `pprof`) sit behind
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
