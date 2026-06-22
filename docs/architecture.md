# Architecture

Architecture overview for embedders and contributors. Snapshot at
v1.1 (target — sprint in progress as of 2026-06-23). For perf
numbers see [`performance.md`](performance.md); for dialect
support see [`compatibility.md`](compatibility.md).

---

## Crate layout

luna ships as a Cargo **workspace** with two publishable crates:

| Crate | Depends on | Surface |
|---|---|---|
| `luna-core` | **0 third-party crates** (only `std`) | Lexer, parser, compiler, interpreter, runtime, stdlib, GC, pattern engine, JIT trait surface |
| `luna` | `luna-core` + Cranelift × 6 | Cranelift JIT backend, capi (`lua_*` C ABI + `cdylib`/`staticlib`), `luna` CLI binary, benches |

The split lets embedders pick the dependency surface:

```toml
# Minimum embedding — interpreter only, no JIT.
# Builds in seconds; pulls only luna-core.
[dependencies]
luna-core = "1.1"
```

```toml
# Full embedding — JIT'd hot loops, cdylib for C/C++ hosts.
[dependencies]
luna = "1.1"
```

`cargo install luna` installs the `luna` CLI binary (REPL + script runner).

The 0-dep `luna-core` is a hard contract enforced by `cargo deny check` in CI:
`cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"` must equal `1`
(luna-core itself, nothing else). This guarantees:

- **Build time**: embedders pulling only `luna-core` skip ~33 transitive Cranelift dependencies.
- **wasm32 compatibility**: `luna-core` builds for `wasm32-unknown-unknown` because JIT (which requires `mmap` RWX) is not pulled in.
- **Audit surface**: security-conscious embedders audit one crate, not the full Cranelift tree.
- **Long-term portability**: changing JIT backend (or removing JIT entirely) is a `luna` crate concern, never touches `luna-core` API.

The JIT is plugged in through a trait (`IntChunkCompiler` / `TraceCompiler`)
defined in `luna-core::jit`. `luna-core` ships a `NullJitBackend` that satisfies
the trait without compiling anything (the dispatcher takes the interpreter path
for every call). The `luna` crate provides `CraneliftBackend` and the convenience
constructor `Vm::new_minimal_with_jit(...)` that installs it at construction.

```rust
// luna-core only — interpreter dispatch all the way.
let mut vm = luna_core::Vm::new_minimal(LuaVersion::Lua55);
vm.open_base();
vm.eval("return 1 + 2")?;

// luna (interpreter + Cranelift JIT) — hot loops compile.
let mut vm = luna::Vm::new_minimal_with_jit(LuaVersion::Lua55);
vm.open_base();
vm.eval("for i = 1, 1e6 do end")?;
```

---

## Source classification

luna's source files fall into three tiers, each with a different change
discipline and review depth. Knowing which tier a file belongs to tells
you how risky a change is and how much testing it needs.

### Stone — business-agnostic foundations

Generic algorithms and protocols. No knowledge of Lua semantics; could be
lifted into another project if the contract holds.

Lives in: `luna-core/src/{numeric,pattern}.rs`, parts of `luna-core/src/runtime/gc.rs`
(the mark-sweep core).

Discipline:
- Heavy unit tests + fuzz harness
- API breaks require version bump + migration note
- Cross-platform behavior verified (wasm32 inclusive)

### Steel — Lua-domain primitives

Knows Lua semantics (the language, the calling convention, the value
model) but not any specific embedder workflow. The structural beams of
the project.

Lives in: `luna-core/src/{compiler,frontend,runtime/value,vm/exec,vm/isa}/`,
`luna-core/src/jit/{abi,trace_types}.rs`.

Discipline:
- Integration tests cross-validate against PUC and LuaJIT reference behavior
- Per-dialect (5.1-5.5) regression tests on every change
- Behavior changes require an ADR / discussion entry

### Cement — concrete embeddings and host glue

Glue between the steel and the outside world. Tightly coupled to a
specific workflow or host integration.

Lives in: `luna/src/{capi,bin}/`, `luna/src/jit_backend/`, benches, examples.

Discipline:
- End-to-end tests in the relevant host shape (CLI, capi, JIT-on real workload)
- Free to evolve as embedder needs change; not API-stable in the same sense
- Bug fixes don't require touching steel

The crate boundary roughly tracks this classification: most of `luna-core`
is steel and stone, most of `luna` is cement. The few cement bits in
`luna-core` (e.g. dialect-specific stdlib glue) are isolated under
`luna-core/src/vm/lib_*.rs` so they can be feature-gated later if needed.

---

## JIT pipeline overview

luna's JIT is a **trace-based** compiler in the LuaJIT lineage. The
pipeline:

```
   Interpreter dispatcher (luna-core/src/vm/exec.rs)
            │
            │ per opcode: bump counter, check threshold
            ▼
   Hot path detected
            │
            │ start recording into TraceRecord
            ▼
   Trace recorder (luna-core/src/vm/exec.rs)
            │
            │ replay opcodes into typed IR ops; stop on side-exit
            ▼
   Closed TraceRecord
            │
            │ hand to chunk_compiler.try_compile_trace(record, opts)
            ▼
   ┌────────────────────────────────────────────────────────┐
   │ luna-core ←─ trait boundary ─→ luna                    │
   │                                                          │
   │ IntChunkCompiler /          ←─── CraneliftBackend       │
   │ TraceCompiler                                            │
   │                                                          │
   └────────────────────────────────────────────────────────┘
            │
            │ luna/src/jit_backend/trace.rs lowers to Cranelift IR
            ▼
   Cranelift codegen
            │
            │ regalloc + machine code emit + mmap RWX + symbol relocation
            ▼
   CompiledTrace { entry: *const u8, exit_tags: ... }
            │
            │ stored under Proto.traces (Rc<CompiledTrace> side-table)
            ▼
   Dispatcher routes future dispatches at same PC into the compiled trace
            │
            │ on guard failure / side-exit: snapshot back, resume interp
            ▼
   Continue interpreting (or trigger side-trace recording)
```

Key properties:

- **Trace dispatch is per-Proto, not per-call**. The vtable cost of going
  through `IntChunkCompiler` lives entirely on the cold path (one call per
  function-prototype, total). Once a `Proto` has a cached `CompiledTrace`,
  the dispatcher reads it directly out of the `Proto.traces` field — no
  trait indirection on the hot path.

- **`CompiledTrace` lives in `luna-core`**. Its layout (`u32` counters,
  `Rc<[ExitTag]>` exit table, `unsafe extern "C" fn(*mut i64) -> i64`
  function pointer) is Cranelift-free; only the body of the function
  pointer it owns comes from Cranelift.

- **Side traces** (compiled paths from frequently-taken side exits)
  attach back into the parent trace's exit table at runtime. This keeps
  branchy code from re-entering the interpreter just because a less-common
  branch is taken occasionally.

- **JIT can be disabled per `Vm`**: `vm.set_jit_enabled(false)` or
  installing `NullJitBackend` (default if you `cargo add luna-core`
  without the `luna` wrapper) makes every dispatch take the interpreter
  path. The interp on its own is competitive with PUC interpreters and
  faster than PUC 5.1-5.5 across the cross-dialect bench (see
  [`performance.md`](performance.md)).

The 26 `luna_jit_*` `extern "C"` helpers (called from Cranelift-emitted
mcode) live in `luna/src/jit_backend/mod.rs`. They're `pub` in the `luna`
cdylib/staticlib so the linker resolves the symbols when JIT-emitted code
calls them. From an embedder's view, these are invisible — the helpers
service the JIT backend, not the public API.

---

## Threading model

`luna_core::Vm` is `!Send + !Sync` by construction. The GC uses
`NonNull<T>` over an intrusive mark-sweep heap (not `Rc<RefCell<T>>`);
the trace JIT side-table uses `Rc<CompiledTrace>`. Both are
single-threaded on purpose — past benches put the cost of an
`Arc`+`RwLock` shape at 5-15% on Redis-Lua-shape workloads.

Embedders wanting concurrency spawn one `Vm` per OS thread (or per
single-thread Tokio worker) and exchange data through channels. Async
embedders use `tokio::main(flavor = "current_thread")` or wrap the `Vm`
in a `LocalSet` under a multi-thread runtime.

A future `feature = "send"` on `luna-core` is on the v1.x post-sprint
roadmap — it would flip `Gc<T> → Arc<RwLock<T>>` behind a hard ≤8%
regression budget. See `.dev/rfcs/v1.1-rfc-vm-send-sync.md` for the
detailed plan.

For canonical embedding patterns and code samples, see
[`threading.md`](threading.md).

---

## Sandbox and embedder surface

The `luna-core::Vm` exposes a builder for sandboxed embedding:

```rust
let mut vm = Vm::sandbox(LuaVersion::Lua55)
    .open_base()
    .open_math()
    .open_string()
    .open_table()
    .with_instr_budget(1_000_000)
    .with_memory_cap(8 * 1024 * 1024)
    .build();
```

The sandbox defaults to:
- Whitelist-style stdlib opening — no `io`, `os`, `debug`, `package` unless explicitly enabled
- Bytecode loading off — `string.dump` / `loadstring` rejected
- JIT off in sandbox mode — JIT-compiled paths are larger attack surface than the interpreter
- Default instruction budget — preventing infinite loops in untrusted scripts

Trusted embedders skip the sandbox builder and use `Vm::new_minimal_with_jit`
(via the `luna` crate) for full speed.

Public API surface contracts:

- **Zero `unsafe` at the embedder surface**. The public API never requires
  the caller to write an `unsafe { ... }` block. `unsafe` exists inside
  the implementation (mark-sweep GC, JIT FFI), each block annotated with
  a `SAFETY:` comment justifying the invariant.

- **No panic on user input**. `Vm::eval` / `Vm::load` / `Vm::call_value`
  all return `Result<_, LuaError>` for any input — malformed source,
  type errors, runtime errors, instruction budget exhaustion. Panics in
  the public API are bugs.

- **Backwards-compatible re-exports**. The `luna` crate's `pub use
  luna_core::*` plus the `pub mod jit { pub use luna_core::jit::*; ... }`
  re-exports keep `use luna::vm::Vm`, `use luna::jit::TraceRecord`,
  `use luna::capi::LuaState` working unchanged from v1.0.

---

## Where to look

| You want to | Read |
|---|---|
| Embed luna in a Rust program | [`embedding.md`](embedding.md) (cookbook) |
| Embed luna in a C/C++ program | `crates/luna/src/capi.rs` (full `lua_*` ABI) |
| Use luna from the CLI | `luna --help` and the examples in [README](../README.md) |
| Understand which Lua features work | [`compatibility.md`](compatibility.md) |
| Compare luna against PUC/LuaJIT | [`performance.md`](performance.md) |
| Run luna in an async runtime | [`threading.md`](threading.md) |
| Hack on the JIT | `crates/luna/src/jit_backend/` |
| Hack on the interpreter | `crates/luna-core/src/vm/exec.rs` |
| Hack on the GC | `crates/luna-core/src/runtime/gc.rs` |
| Hack on a stdlib library | `crates/luna-core/src/vm/lib_*.rs` |
