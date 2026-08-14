# luna

A Lua runtime in pure Rust. Five dialects — **5.1 / 5.2 / 5.3 / 5.4 /
5.5** — plus MacroLua in a single binary, with a zero-dependency
interpreter core, a Cranelift-backed trace JIT, ahead-of-time native
compilation, and a sandbox built for embedding untrusted scripts.

**[luna.golia.jp](https://luna.golia.jp)** · [docs](https://luna.golia.jp/docs.html) · [crates.io](https://crates.io/crates/luna-jit) · [docs.rs](https://docs.rs/luna-jit)

```rust
use luna_jit::{Vm, LuaVersion};

let mut vm = Vm::new(LuaVersion::Lua54);
let v = vm.eval("return 6 * 7")?;   // [Int(42)]
```

## Status

**v3.0.0** shipped 2026-08-14, closing the v2.x maturity arc. The major
bump marks a maturity gate rather than an API break: the public surface
is identical to 2.18.0 — 788 items, zero removals, zero additions — and
code written against 2.x compiles against 3.0 unchanged. See
[`CHANGELOG.md`](CHANGELOG.md) for what the arc bought and what each of
its ten acceptance criteria rests on today.

## Install

Seven crates are published; three of them are ones you depend on
directly.

```toml
# Most embedders — interpreter + Cranelift JIT + the C ABI.
[dependencies]
luna-jit = "3"
```

```toml
# Minimum surface — pure interpreter, zero third-party deps, wasm-friendly.
[dependencies]
luna-core = "3"
```

`cargo tree -p luna-core` prints exactly one crate: itself. A CI gate
enforces that on every commit, so the audit surface stays small and the
`wasm32` target needs no RWX mapping. `luna-jit` adds Cranelift.

For a standalone native binary from a Lua source file, `luna-aot` is a
build-time tool — not a runtime dependency of what it produces. See
[`docs/aot.md`](docs/aot.md).

The CLI:

```sh
cargo install luna-jit                              # `luna` REPL + script runner
cargo install luna-jit --features repl-line-editor  # + completion and highlighting
```

## Dialects

One build hosts every mainline dialect, selected per-`Vm` at
construction; a single process can run several concurrently without
interference. Feature availability is driven by capability predicates in
`crates/luna-core/src/version.rs`, and luna emits per-dialect bytecode
in PUC's format, so PUC-compiled `.luac` files load directly.

`MacroLua` is a sixth dialect: the 5.4 surface plus compile-time
`@macro(...)` expansion. It sits between `Lua54` and `Lua55` in the
version enum so it inherits every 5.4-and-earlier capability predicate.

Full matrix: [`docs/compatibility.md`](docs/compatibility.md).

## Embedding

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

let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
lua.set_global("add", add)?;

let result: i64 = lua.eval("return add(40, 2)")?;
assert_eq!(result, 42);
```

The host owns the security boundary: curated stdlib whitelisting, an
instruction budget, an approximate memory cap, and bytecode loading off
by default. Every capability a script can see was opted into from Rust.

No API an embedder touches requires an `unsafe` block.

Cookbook: [`docs/embedding.md`](docs/embedding.md). Threat model and
what is explicitly *not* contained:
[`docs/security.md`](docs/security.md).

> **Looking for embedders.** luna wants production users and the
> feedback that comes with them. See
> [`docs/embedder-recruitment.md`](docs/embedder-recruitment.md) for
> what it offers, what it does not, and how to try it.

## Correctness

Compatibility here is a measurement, not a claim.

- **514 differential fixtures** run against stock PUC interpreters built
  from source — 5.1.5, 5.2.4, 5.3.6, 5.4.8, 5.5.1 — and must match
  byte for byte on stdout, stderr and exit code, with zero skips, before
  a commit is green. CI additionally asserts the 5.5 reference is
  exactly 5.5.1, so the basis cannot drift with a runner image.
- **PUC's own test suite** runs end-to-end across all five dialects with
  matching assert-count instrumentation.
- **AddressSanitizer** over that suite nightly; **Miri** for provenance
  and UB; **cross-allocator** runs on glibc, jemalloc, mimalloc and
  Apple malloc.
- **Seven fuzz targets** weekly, behind a gate that fails the run if any
  target leaves a crash artifact.
- **Soak** runs bounded on second-half RSS drift under 1%.
- The cross-platform matrix — ubuntu / macos / windows / ubuntu-arm ×
  stable, plus wasm32 — runs per push.

One lesson from the arc is worth repeating: a green check is not
evidence that something ran. v2.17 found three gates that had never
executed, and v2.20 two CI jobs that had never completed, all of them
reporting success throughout.

## Performance

luna deliberately does not publish a headline ratio. A single "N× faster
than X" collapses two independent axes — `luna_jit vs LuaJIT_jit` and
`luna_interp vs LuaJIT_interp` — and the number moves with the corpus it
was measured on. [`docs/performance.md`](docs/performance.md) gives the
methodology, what is measured today, and the per-release perf gate,
which compares HEAD against a pinned reference commit **on the same
runner** rather than against fixed nanosecond baselines that drift with
CI hardware.

## Threading

`Vm` is `!Send + !Sync` — pin one per OS thread, or per single-threaded
Tokio worker. For async hosts use the `current_thread` flavour or a
`LocalSet`. [`docs/threading.md`](docs/threading.md) has the three
canonical patterns and the reasoning behind the constraint.

## Standalone CLI

```sh
luna script.lua
luna -e "print(1 + 2)"
luna --lua=5.4 script.lua     # pick a dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5)
luna --sandbox script.lua     # safe stdlib subset; reject bytecode loading
luna --budget=1000000 s.lua   # cap dispatched instructions
luna --no-jit script.lua      # interpreter only
luna --profile script.lua     # print trace-JIT counters on exit
luna                          # interactive REPL (Ctrl-D exits)
```

## Linking from C

`luna-jit` builds a `cdylib` / `staticlib` exposing a `lua.h`-compatible
subset (`crates/luna-jit/src/capi.rs`), for C and C++ hosts that want a
drop-in replacement for PUC.

## Build

```sh
cargo build --release --workspace
cargo test --release --workspace
cargo bench --bench cross_dialect     # vs PUC and LuaJIT
cargo bench --bench redis_lua_shape   # Redis-Lua embedder shapes
```

## Architecture

```
crates/
├── luna-core/              # 0 third-party deps — lexer, parser, per-dialect
│                           #   compiler, dispatcher, stdlib, NaN-boxed values,
│                           #   intrusive mark-sweep GC, PUC pattern engine,
│                           #   and the JIT trait surface
├── luna-jit/               # Cranelift backend, lua.h-compatible C ABI,
│                           #   the `luna` CLI, the Lua embedding facade
├── luna-jit-derive/        # #[derive(LuaUserdata)] — kept separate so
│                           #   luna-core stays zero-dependency
├── luna-jit-helpers/       # shared extern-C helpers and JIT TLS discipline,
│                           #   single-sourced across both backends
├── luna-jit-llvm/          # alternative LLVM 18 backend (LUNA_JIT_BACKEND=llvm)
├── luna-runtime-helpers/   # staticlib linked into AOT-produced binaries
└── luna-aot/               # build-time: Lua source → standalone native binary
```

The JIT plugs in behind a trait, so the interpreter path never depends
on a backend and removing one does not touch the core API.
[`docs/architecture.md`](docs/architecture.md) has the full breakdown.

## Documentation

| | |
|---|---|
| [`docs/embedding.md`](docs/embedding.md) | the cookbook |
| [`docs/compatibility.md`](docs/compatibility.md) | per-dialect feature matrix |
| [`docs/architecture.md`](docs/architecture.md) | crate boundaries, JIT pipeline |
| [`docs/security.md`](docs/security.md) | sandbox boundaries and threat model |
| [`docs/threading.md`](docs/threading.md) | async and multi-thread patterns |
| [`docs/aot.md`](docs/aot.md) | ahead-of-time compilation |
| [`docs/deploy.md`](docs/deploy.md) | shipping something that embeds luna |
| [`docs/performance.md`](docs/performance.md) | methodology and measurements |
| [`docs/binary-size.md`](docs/binary-size.md) | what the binary costs |
| [`docs/unsafe-accounting.md`](docs/unsafe-accounting.md) | every `unsafe` site, justified |
| [`docs/migration-v1-to-v2.md`](docs/migration-v1-to-v2.md) | v1.x → v2.x migration |
| [`docs/release-checklist.md`](docs/release-checklist.md) | how a release is cut |
| [`CHANGELOG.md`](CHANGELOG.md) | release notes |

Rendered API reference: [docs.rs/luna-jit](https://docs.rs/luna-jit) and
[docs.rs/luna-core](https://docs.rs/luna-core), or `cargo doc --open`.
Every public item is documented — `deny(missing_docs)` enforces it.

## License

Dual MIT / Apache-2.0 (see [`LICENSE-MIT`](LICENSE-MIT) and
[`LICENSE-APACHE`](LICENSE-APACHE)).
