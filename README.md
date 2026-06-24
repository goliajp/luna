# luna

A Lua runtime in pure Rust. Full Lua **5.1 / 5.2 / 5.3 / 5.4 / 5.5**
dialect support in a single binary, with a Cranelift-backed JIT
that hits **2× faster than PUC** across the cross-dialect bench and
**competitive with LuaJIT 2.1** on numeric workloads.

```rust
use luna_jit::Lua;

let mut lua = Lua::new();
lua.open_base();
lua.open_math();
let r: i64 = lua.eval("return 1 + 2")?;
assert_eq!(r, 3);
```

## Status

- **v1.0.0** shipped 2026-06-23 (commit `ae7795c`).
- **v1.1** sprint in progress: ergonomic embedder API
  (`Lua::eval` / `set_global` / `native_typed` / `LuaTable` / etc.),
  workspace split (`luna-core` is zero-dep), MSRV declaration,
  structured `LuaError`, host userdata payloads, Rust-side
  coroutine + debug hook APIs, and async embedder integration
  (see [`CHANGELOG.md`](CHANGELOG.md) for the full unreleased
  section).

### Correctness

- **910 tests / 0 failures** across the v1.0 baseline (242 lib unit,
  123 PUC official-suite files × 5 dialects, 40 e2e programs × 5
  dialects byte-diff vs installed PUC, 64 method-JIT audit, 28
  trace-JIT audit, 13 capi conformance, 10 sandbox embedding, etc.)
- v1.1 adds **60+ new integration tests** for the embedder ergo
  surface (`sandbox_builder`, `table_builder`, `native_typed`,
  `lua_facade`, `userdata_host`, `rust_debug_hook`, `rust_coroutine`,
  `lua_error_structured`).

### Performance

Master gate is `vs.X ≤ 0.50` — luna at least 2× faster than the
reference — on every cross-dialect cell:

| | cells | pass |
|---|---:|---:|
| vs PUC 5.1-5.5 (7 cells × 5 dialects) | 35 | **35** ✓ |
| vs LuaJIT 2.1 (7 cells) | 7 | 6 ✓ (1 within 0.83×) |

See [`docs/performance.md`](docs/performance.md) for the full table
and the Redis-Lua-shape (D1) corpus.

## Install

luna ships as a Cargo workspace with two publishable crates:

```toml
# Most embedders — full interp + Cranelift JIT + capi.
[dependencies]
luna-jit = "1.1"
```

```toml
# Minimum surface — pure interpreter, zero third-party deps,
# wasm32-friendly.
[dependencies]
luna-core = "1.1"
```

`cargo tree -p luna-core` shows exactly one crate. The full `luna`
adds 6 Cranelift crates and their transitive deps for the JIT side.

For the CLI binary:

```sh
cargo install luna-jit   # `luna` REPL + script runner
# or, for the polished REPL (tab completion against globals + Lua
# syntax highlighting; pulls rustyline as a dep):
cargo install luna-jit --features repl-line-editor
```

## Embedding (quick demo)

The sandbox builder + ergo APIs collapse the v1.0 dance into a
handful of lines:

```rust
use luna_jit::Lua;
use luna_jit::version::LuaVersion;

let mut lua = Lua::sandbox(LuaVersion::Lua54)
    .open_base()
    .open_math()
    .open_string()
    .with_instr_budget(1_000_000)
    .with_memory_cap(8 * 1024 * 1024)
    .build();

// Register a typed Rust function:
let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
lua.set_global("add", add)?;

// Build a table and expose it:
let cfg = lua.create_table();
cfg.set(&mut lua, "answer", 42_i64)?;
cfg.set(&mut lua, "name", "luna")?;
lua.set_global("cfg", cfg)?;

// Run a script:
let result: i64 = lua.eval("return add(cfg.answer, 8)")?;
assert_eq!(result, 50);
```

Full walkthrough: [`docs/embedding.md`](docs/embedding.md) (12
sections covering install, sandbox, set_global, tables, native
functions, userdata, coroutines, debug hooks, errors, the `Lua`
newtype facade, and threading).

## Threading model

`luna_jit::Vm` is `!Send + !Sync` — pin one Vm per OS thread (or per
single-thread Tokio worker). For async embedders, use Tokio's
`current_thread` runtime flavor or wrap `Vm` access in a `LocalSet`.
See [`docs/threading.md`](docs/threading.md) for canonical patterns
(single-thread Tokio, `LocalSet` on multi-thread, `Vm`-per-OS-thread
+ channels) and the post-v1.1 `feature = "send"` roadmap.

## Standalone CLI

```sh
cargo run --release --bin luna -- -e "print('hello, world')"
cargo run --release --bin luna -- path/to/script.lua
```

`luna --version-of <51|52|53|54|55>` selects the dialect.

## Linking from C

`luna` ships a `cdylib` / `staticlib` exposing a `lua.h`-compatible
subset under `crates/luna-jit/src/capi.rs`. Existing C / C++ hosts that
need a drop-in PUC replacement can link against it.

## Build

```sh
cargo build --release --workspace
cargo test --release --workspace      # full suite, ~30 s
cargo bench --bench cross_dialect     # microbench vs PUC + LuaJIT
cargo bench --bench redis_lua_shape   # Redis-Lua embedder shapes
```

## Architecture in 30 seconds

```
crates/luna-core/        # 0 third-party deps; pure interp + types
├── src/vm/              # dispatcher + sandbox + ergo (eval, native_typed, ...)
├── src/runtime/         # heap (NonNull-based mark-sweep GC), value, table, userdata
├── src/compiler/        # bytecode emit per dialect
├── src/frontend/        # lexer + parser
├── src/pattern.rs       # PUC pattern engine
├── src/jit/             # IntChunkCompiler + TraceCompiler trait surface
│                        # (NullJitBackend lives here; embedders compose
│                        #  their own implementations against this contract)
└── src/lib.rs           # module roots

crates/luna-jit/             # depends on luna-core + cranelift × 6
├── src/jit_backend/     # Cranelift-backed CraneliftBackend implementations
├── src/capi.rs          # lua.h-compatible C ABI
├── src/lua_facade.rs    # `Lua` newtype mlua-shape facade
├── src/bin/luna.rs      # CLI binary
└── src/lib.rs           # pub use luna_core::*; + JIT + Vm::new_minimal_with_jit
```

See [`docs/architecture.md`](docs/architecture.md) for the full
breakdown (crate boundary, source classification, JIT pipeline,
sandbox surface).

## Documentation

- [`docs/embedding.md`](docs/embedding.md) — cookbook
- [`docs/architecture.md`](docs/architecture.md) — crate layout +
  JIT pipeline
- [`docs/threading.md`](docs/threading.md) — async + multi-thread
  patterns
- [`docs/compatibility.md`](docs/compatibility.md) — per-dialect
  feature matrix
- [`docs/performance.md`](docs/performance.md) — bench numbers
- [`CHANGELOG.md`](CHANGELOG.md) — release notes
- `cargo doc --open` — full API reference

## License

Dual MIT / Apache-2.0 (see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE)).
