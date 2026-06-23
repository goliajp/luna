# Binary size

Snapshot of `luna` (the CLI binary) release-build composition, via
`cargo bloat`. Refreshed when crate boundaries change. v1.1 sprint
snapshot:

## Total

- `.text` section size: **2.2 MiB**
- File size on disk: **3.4 MiB** (includes debug section, headers, etc.)
- `cargo bloat --release -p luna --bin luna --crates` aggregation
  below.

## By crate

| Share of .text | Size | Crate |
|---:|---:|---|
| 45.1% | 1.0 MiB | `cranelift_codegen` |
| 25.5% | 585 KiB | `luna_core` |
| 13.3% | 305 KiB | `std` |
| 7.2% | 165 KiB | `luna` |
| 2.7% | 62 KiB | `regalloc2` |
| 2.3% | 53 KiB | `cranelift_frontend` |
| 1.4% | 32 KiB | `cranelift_jit` |
| 1.4% | 31 KiB | `gimli` |
| <1% each | various | `smallvec`, `cranelift_bforest`, `anyhow`, `target_lexicon`, etc. |

**Embedders that don't need the JIT** import `luna-core` instead of
`luna` — `cargo tree -p luna-core --prefix none | grep -cE ' v[0-9]'`
returns 1, and the resulting `rlib` is roughly the 585 KiB
`luna_core` slice above (no Cranelift / regalloc2 / etc. linkage).

## Top functions by size

| Share | Size | Function |
|---:|---:|---|
| 6.5% | 149 KiB | `cranelift_codegen::opts::...constructor_simplify` |
| 2.5% | 57 KiB | `luna_jit::jit_backend::trace::try_compile_trace_with_options` |
| 2.3% | 53 KiB | `cranelift_codegen::isa::aarch64::...constructor_lower` |
| 2.0% | 45 KiB | `luna_jit::jit_backend::try_compile_int_chunk` |
| 1.6% | 36 KiB | `AArch64Backend::compile_function` |
| 1.5% | 33 KiB | `luna_core::vm::exec::Vm::run` (the interp dispatcher) |
| 1.2% | 26 KiB | `luna_core::compiler::Compiler::stat_block_inner` |
| 1.2% | 26 KiB | `cranelift_codegen::machinst::compile::compile` |
| 1.1% | 26 KiB | `regalloc2::ion::run` |
| 1.1% | 25 KiB | `regalloc2::ion::init` |

## Reproducing

```sh
cargo install --locked cargo-bloat
cargo bloat --release -p luna --bin luna --crates -n 20
cargo bloat --release -p luna --bin luna -n 20
```

Cranelift dominates because it's a full-featured production JIT
(not a tiny baseline JIT). Embedders willing to ship interp-only
hit `luna-core` and drop ~1 MiB of `.text`.

## Methodology notes

- Numbers above are guesswork — `cargo bloat` reads symbol sizes
  from the linker output, not perfectly correlated with source
  line counts. Use as relative comparison, not absolute audit.
- Run the same command on multiple platforms — Apple Silicon
  shows `aarch64` Cranelift codegen; x86_64 hosts show `x86_64`
  paths instead.
- Release LTO is enabled by default in luna's `[profile.release]`
  (see `Cargo.toml`) so the numbers reflect a fully-optimized
  binary, not a debug build.

For dependency tree, see `cargo tree -p luna` (~39 transitive
crates, all Cranelift-linked) vs `cargo tree -p luna-core` (1 crate
exactly).
