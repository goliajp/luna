# luna-fuzz — v2.0 Track CV fuzz harnesses

Scaffold-level fuzz targets for the four highest-value attack surfaces
identified in the v2.0 Phase 0 Track CV audit:

| target              | input               | code under test                                 |
|---------------------|---------------------|-------------------------------------------------|
| `fuzz_parser`       | random bytes        | `luna_core::frontend::parser::parse` × 6 dialects |
| `fuzz_dump_reader`  | random bytes        | `luna_core::vm::dump::undump` (luna + PUC 5.1-5.5) |
| `fuzz_vm_dispatch`  | random UTF-8 source | full eval pipeline (parser + compiler + dispatcher) |
| `fuzz_aot_meta`     | random bytes        | `luna_core::jit::aot_meta::decode_meta_blob` + per-byte unpackers |

## Why workspace-excluded

`luna-fuzz` is **not** a workspace member. The root `Cargo.toml`
[workspace] table excludes `crates/luna-fuzz` so:

* default `cargo build --workspace` does NOT pull `libfuzzer-sys`,
* the nightly toolchain pin in `rust-toolchain.toml` here does NOT
  affect sibling crates,
* the `luna-core` 0-third-party-dep contract enforced by the
  `zero-dep` CI job stays trivially satisfied (the fuzz crate is
  off the dep graph for everything else).

## Running

Requires `cargo-fuzz` and a nightly toolchain (auto-selected by
`rust-toolchain.toml`):

```
cargo install cargo-fuzz       # one-time
cd crates/luna-fuzz
cargo +nightly fuzz run fuzz_parser            # interactive — Ctrl-C to stop
cargo +nightly fuzz run fuzz_parser -- -max_total_time=300   # 5-min batch
cargo +nightly fuzz build                      # compile all targets (sanity)
```

CI runs each target for 5 min on every PR (smoke) and 60 min weekly
on cron (deeper). See `.github/workflows/fuzz.yml`.

## No try-catch / panic-trap

Per `code/no-blind-bugfix-pattern`: harnesses do NOT wrap targets in
`std::panic::catch_unwind`. A panic IS a real bug — libfuzzer-sys
captures it as a crash input we want to ship a regression test for.

## Per-track content fills

This scaffold ships **harness skeletons only**. The v2.0
implementation phase per-track CV-content sprints will add:

* seed corpora (representative valid inputs to bootstrap libfuzzer's
  coverage feedback),
* `corpus/` and `artifacts/` directories with crash inputs found in
  fuzz campaigns,
* regression entries in the per-crate `tests/regressions/` directory
  for each historical crash.
