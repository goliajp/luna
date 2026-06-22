# luna

A Lua runtime in pure Rust. Full support for **Lua 5.1 / 5.2 / 5.3 / 5.4
/ 5.5** in a single binary (5.5 is the primary dialect). Zero non-build
dependencies — cranelift is the JIT codegen.

Designed for embedding in script hosts. The top-level surface is a
sandbox-friendly `Vm` with whitelisted libraries, per-call instruction
and memory budgets, and host-registered native callbacks. The cranelift
JIT is on by default for performance and can be disabled by embedders
that need every dispatch turn to tick the budget.

## Status

**v1.0.0** released 2026-06-23. Stable embedding + C API per semver;
internal modules (JIT codegen, dispatcher hot path internals) remain
subject to optimization but the public surface (`Vm`, `Value`,
`LuaVersion`, `capi`) is frozen for the 1.x line.

### Correctness

- **910 tests passing, 0 failures, 0 ignored**
- Official PUC test suite: **123 / 123 expected-pass files** across
  Lua 5.1 (23), 5.2 (26), 5.3 (27), 5.4 (32), 5.5 (15)
- 40 end-to-end Lua programs × 5 dialects produce byte-identical
  stdout vs the installed PUC reference binary
- 64 method-JIT × dialect × `Value`-introspection audit tests
- 28 trace-JIT audit tests covering hot loops, recursion, side
  traces, pcall, etc.

### Performance

Cross-dialect microbench on M-series (in-process; PUC + LuaJIT
times include subprocess startup):

- vs PUC 5.1-5.5: **35 / 35 cells pass the ≥ 2× master gate**
- vs LuaJIT 2.1: **6 / 7 cells pass** (binary_trees_n10 at 0.83×
  — luna is 1.21× faster than LuaJIT but doesn't clear 2×; this is
  the design ceiling under luna's no-NaN-boxing + PUC bytecode
  compatibility constraints)

Full snapshot at `docs/performance.md`.

## Install

Add to `Cargo.toml`:

```toml
[dependencies]
luna = "1.0"
```

## Usage

### Embedding (sandbox host)

```rust
use luna::version::LuaVersion;
use luna::vm::Vm;
use luna::runtime::Value;

let mut vm = Vm::new_minimal(LuaVersion::Lua51);

// Whitelist: no os/io/debug/package.
vm.open_base();
vm.open_math();
vm.open_string();
vm.open_table();
vm.open_coroutine();

// Sandbox gates (required when running untrusted scripts).
vm.set_jit_enabled(false);
vm.set_bytecode_loading(false);

// Quotas.
vm.set_instr_budget(Some(100_000));
vm.set_memory_cap(Some(1 << 20));

let cl = vm.load(b"return 1 + 2", b"=eval").expect("compile");
let result = vm.call_value(Value::Closure(cl), &[]).expect("run");
assert!(matches!(result.as_slice(), [Value::Int(3) | Value::Float(_)]));
```

Run the included walkthrough:

```sh
cargo run --release --example sandbox_demo
```

See `cargo doc --open` (top-level `src/lib.rs` rustdoc) for the full
embedding contract and sandbox caveats.

### Threading model

`luna::Vm` is `!Send + !Sync` — pin one Vm per OS thread (or per
single-thread Tokio worker). For async embedders, use Tokio's
`current_thread` runtime flavor or wrap `Vm` access in a `LocalSet`.
See [`docs/threading.md`](docs/threading.md) for canonical patterns
(single-thread Tokio, `LocalSet` on multi-thread, `Vm`-per-OS-thread
+ channels) and the post-v1.1 `feature = "send"` roadmap.

### Standalone CLI

```sh
cargo run --release --bin luna -- -e "print('hello, world')"
cargo run --release --bin luna -- path/to/script.lua
```

`luna --version-of <51|52|53|54|55>` selects the dialect.

### Linking from C

`luna` ships a `cdylib` / `staticlib` exposing a `lua.h`-compatible
subset under `src/capi.rs`. Existing C / C++ hosts that need a
drop-in PUC replacement can link against it.

## Build

```sh
cargo build --release
cargo test --release               # full suite, ~30 s
cargo bench --bench cross_dialect  # microbench (needs PUC + LuaJIT on PATH)
```

## Documentation

- `docs/compatibility.md` — embedder compatibility surface (dialect
  feature matrix, stdlib coverage, C API surface, bytecode compat)
- `docs/performance.md` — perf vs PUC 5.1-5.5 and LuaJIT 2.1
- `CHANGELOG.md` — release notes
- `cargo doc --open` — full API reference

## License

Dual-licensed under either of:

- MIT License (`LICENSE-MIT`)
- Apache License, Version 2.0 (`LICENSE-APACHE`)

at your option.
