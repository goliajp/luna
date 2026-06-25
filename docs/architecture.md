# Architecture

Architecture overview for embedders and contributors. Snapshot at
v1.3 (shipped 2026-06-25) with v2.0 sprint annotations. For perf
methodology see [`performance.md`](performance.md); for dialect
support see [`compatibility.md`](compatibility.md); for the deploy-
side decision tree see [`deploy.md`](deploy.md).

---

## Crate layout

luna ships as a Cargo **workspace** with five publishable crates
plus two dev-only members:

| Crate | Publishable | Depends on | Surface |
|---|---|---|---|
| `luna-core` | ✅ | **0 third-party crates** (only `std`) | Lexer, parser, compiler, interpreter, runtime, stdlib, GC, pattern engine, JIT trait surface |
| `luna-jit-derive` | ✅ | `syn` + `quote` + `proc-macro2` | `#[derive(LuaUserdata)]` proc-macro |
| `luna-jit` | ✅ | `luna-core` + `luna-jit-derive` + Cranelift × 6 + rustyline (opt) | Cranelift JIT backend, capi (`lua_*` C ABI), `luna` CLI (REPL + script runner), JIT-aware embed |
| `luna-runtime-helpers` | ✅ | `luna-jit` (behind `jit-helpers` feature) | Static-link runtime entry for AOT-produced binaries. Exposes `luna_aot_run` C-ABI symbol |
| `luna-aot` | ✅ | `luna-core` + `luna-jit` + Cranelift × 6 + `object` + `clap` | Build-time AOT compiler. Lua source → standalone native binary. Not a runtime dep of the produced binary |
| `luna-fuzz` | ❌ workspace-excluded | `libfuzzer-sys` + `luna-core` | Fuzz harnesses (4: parser / dump / vm / aot_meta). Nightly toolchain. v2.0 Track CV |
| `luna-tools` | ❌ in-flight | `clap` + `serde` + `object` + opt `pprof` / `capstone` / `inferno` | Dev tools: `luna-bin-inspect` / `luna-heap-dump` / `luna-profile` / `luna-trace-inspect` / REPL polish. v2.0 Track TL |

The split lets embedders pick the dependency surface:

```toml
# Minimum embedding — interpreter only, no JIT.
# Builds in seconds; pulls only luna-core. WASM-friendly.
[dependencies]
luna-core = "1.3"
```

```toml
# Full embedding — JIT'd hot loops, derive macros, REPL.
[dependencies]
luna-jit = "1.3"
```

```toml
# Cross-thread fleet (tokio / web workers).
[dependencies]
luna-jit = { version = "1.3", features = ["send"] }
```

`cargo install luna-jit` installs the `luna` CLI binary (REPL +
script runner). `cargo install luna-aot` installs the `luna-aot`
build-time tool. `cargo install luna-tools` (in flight per v2.0
charter Track TL) installs the dev-tools binaries.

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
let mut vm = luna_jit::Vm::new_minimal_with_jit(LuaVersion::Lua55);
vm.open_base();
vm.eval("for i = 1, 1e6 do end")?;
```

---

## Source classification

luna's source files fall into three tiers, each with a different change
discipline and review depth. Knowing which tier a file belongs to tells
you how risky a change is and how much testing it needs.

### Stone — business-agnostic foundations

Generic algorithms and protocols. No knowledge of Lua semantics;
could be lifted into another project if the contract holds.

Lives in: `luna-core/src/runtime/string_match.rs` (PUC pattern
engine), `luna-core/src/runtime/heap.rs` (intrusive mark-sweep
core), `luna-core/src/runtime/value.rs` (NaN-boxed Value layout),
`luna-core/src/runtime/string.rs` (UTF-8 + interning).

Discipline:
- Heavy unit tests + fuzz harness (`crates/luna-fuzz/fuzz_targets/`
  exercises parser, dump reader, vm dispatcher, aot meta against
  random inputs — `cargo +nightly fuzz run` per target)
- API breaks require version bump + migration note
- Cross-platform behavior verified (wasm32 inclusive)
- Bench + heap baselines: `.dev/baselines/mem-2026-06-25/` +
  `.dev/baselines/disk-2026-06-25/` pin v1.3-ship numbers

### Steel — Lua-domain primitives

Knows Lua semantics (the language, the calling convention, the
value model) but not any specific embedder workflow. The structural
beams of the project.

Lives in: `luna-core/src/{compiler,frontend}/`,
`luna-core/src/vm/{exec,isa,dump}/`, `luna-core/src/jit/`
(trait surfaces + AOT meta types), plus the JIT backend in
`luna-jit/src/jit_backend/`.

Discipline:
- Integration tests cross-validate against PUC and LuaJIT reference
  behavior (`crates/luna-core/tests/official_run.rs` runs 140 PUC
  test files; CB-or wrapper at `.dev/rfcs/v2.0-cb-or-coverage-report.md`
  pins ≥80% per-file assert hit rate as the v2.0 floor)
- Per-dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5 / MacroLua) regression
  tests on every change
- Behavior changes require an audit / RFC entry in `.dev/rfcs/`

### Cement — concrete embeddings and host glue

Glue between the steel and the outside world. Tightly coupled to
a specific workflow or host integration.

Lives in: `luna-core/src/vm/lib_*.rs` (per-stdlib bindings),
`luna-jit/src/{capi,bin}/` (C ABI + `luna` CLI), `luna-jit-derive/`,
`luna-runtime-helpers/`, `luna-aot/`, `luna-tools/` (v2.0 Track TL),
benches, examples.

Discipline:
- End-to-end tests in the relevant host shape (`crates/luna-aot/tests/`
  for AOT, `crates/luna-jit/examples/` for embed cookbooks,
  `crates/luna-jit/benches/` for perf)
- Free to evolve as embedder needs change; not API-stable in the
  same sense as the steel/stone tier (the `pub` surface still
  follows semver; this just means breaking changes here cost less)
- Bug fixes don't require touching steel
- v2.0 Track SQ refactor (audit at `.dev/rfcs/v2.0-audit-source-quality.md`)
  consolidates the cement layer's directory layout per this
  classification — sequenced LAST so R/PI/AO refactors don't
  invalidate the layout decisions

The crate boundary roughly tracks this classification:

- **Stone** = `luna-core` runtime / value / pattern (~30% of luna-core LOC)
- **Steel** = `luna-core` compiler / frontend / vm dispatch + `luna-jit` JIT
  backend (~50% of luna-core LOC, all of luna-jit's JIT pipeline)
- **Cement** = `luna-jit` CLI + capi + `luna-jit-derive` + `luna-runtime-helpers`
  + `luna-aot` + `luna-tools` + per-stdlib `lib_*.rs` glue

The few cement bits in `luna-core` (dialect-specific stdlib glue
under `luna-core/src/vm/lib_*.rs`) are isolated so they could be
feature-gated later if a wasm-friendly minimal embed needs them
out. See `~/.claude-shared/global/methodology/steel-cement-stone.md`
for the full methodology + workflow tier transitions.

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

## AOT pipeline (`luna-aot`)

The `luna-aot` crate compiles a Lua source file to a self-contained
native binary at build time. The produced binary embeds the bytecode in
a `.luna.bytecode` ELF / Mach-O / PE section, statically links against
`luna-runtime-helpers` (a thin staticlib around `luna-core` + `luna-jit`),
and optionally embeds AOT-lowered trace mcode in a `luna_trace_meta` +
`luna_trace_blob` section pair so hot loops dispatch into native code
without the runtime JIT warmup.

The pipeline:

```
   foo.lua
     │  parse + compile + dump (luna-core/src/vm/dump)
     ▼
   bytecode bytes
     │  embed into .luna.bytecode section (luna-aot/src/embed.rs)
     ▼
   foo.luna_bytecode.o
     │
     │  + warmup Vm runs the chunk; trace recorder captures
     │    closed TraceRecords (luna-aot/src/embed.rs::
     │    harvest_and_emit_aot_traces)
     ▼
   foo.luna_traces.o   (one .o per cross target, codegen'd via
                        the target's Cranelift TargetIsa — see below)
     │
     │  + libluna_runtime_helpers.a (built per target via cargo)
     │  + cmain.c stub linking the brackets
     ▼
   cc / cross-cc link
     ▼
   foo   (runnable on the target triple)
```

### Cross-compile model

`luna-aot` cross-compiles by parameterising every stage on a
`TargetSpec` resolved from a triple string (`--target
x86_64-unknown-linux-musl`):

- **Object format**: `TargetSpec.{format, arch, endian}` drives
  `object::write::Object::new` → the bytecode .o and trace .o land with
  the target's magic.
- **Staticlib build**: `cargo build -p luna-runtime-helpers
  --target=<triple>` produces the per-target `.a`.
- **C cmain**: `TargetSpec::cc_command()` picks the right cc driver
  (`cc -target ...` for Apple cross-darwin, `<triple>-gcc` for GNU
  cross-cc, `musl-gcc` for Alpine deploys).
- **Trace mcode** (Stage 7 polish 4): `TargetSpec::cranelift_isa_builder()`
  returns the Cranelift `TargetIsa` for the deploy triple — `x86_64`,
  `aarch64`, `s390x`, `riscv64` — so the offline trace lowerer codegens
  for the deploy ABI, not the host's. The warmup Vm still runs on the
  build host (we can't dispatch target mcode at record time), but the
  captured `TraceRecord`s are luna-IR-level shape (op + guard + reg
  moves) and re-lower correctly for any target — pointer width and
  endianness are encoded as Cranelift `I64` / little-endian, stable
  across every triple in our tier set.

The `cranelift-codegen = { features = ["all-arch"] }` dep pulls in
every backend; without it, a `cargo build` on an aarch64 host would only
link arm64 codegen tables and `isa::lookup` for the cross triple would
return `SupportDisabled`. The extra backends add ~1 MB to luna-aot's
binary; luna-aot is a build-time tool so deployed binaries are
unaffected.

### Cross-compile limitations

- **Helper symbols are target-built**: the `luna_jit_*` helpers the
  trace mcode calls live in `libluna_runtime_helpers.a`, which is
  re-built per target. Cranelift emits the calls as `Linkage::Import`
  C-ABI calls, and the static linker resolves them at link time against
  the target staticlib. Mixing host + target staticlibs would fail at
  link time with `undefined reference`.
- **Host warmup limits the trace set**: only traces that **close at
  warmup time on the build host** can land in the produced AOT mcode.
  If the chunk's hot path requires runtime CLI args / env vars / host
  state, the warmup may close zero traces and the produced binary
  falls back to interp + runtime JIT. This is a soft limitation
  (correctness is unchanged; only the AOT fast-path is missing).
- **MSVC link path not wired**: `*-pc-windows-msvc` targets return a
  clear `AotError::Link` directing the user at `*-pc-windows-gnu` (MinGW)
  instead. MSVC's link.exe is a separate driver-surface we haven't
  bridged.
- **Trace dispatch verification on cross targets**: the
  `stage7_aot_cross_compile_traces` test verifies non-empty
  `luna_trace_meta` in the cross-built binary (i.e. AOT mcode landed),
  but doesn't run the binary — that needs qemu / docker / Rosetta. The
  Stage 6 Alpine docker smoke test exercises the bytecode-interp path
  end-to-end on a different arch; folding AOT-trace dispatch into the
  same docker run is the natural follow-up.

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
  re-exports keep `use luna_jit::vm::Vm`, `use luna_jit::jit::TraceRecord`,
  `use luna_jit::capi::LuaState` working unchanged from v1.0.

---

## Where to look

| You want to | Read |
|---|---|
| Embed luna in a Rust program | [`embedding.md`](embedding.md) (cookbook) |
| Embed luna in a C/C++ program | `crates/luna-jit/src/capi.rs` (full `lua_*` ABI) |
| Use luna from the CLI | `luna --help` and the examples in [README](../README.md) |
| Understand which Lua features work | [`compatibility.md`](compatibility.md) |
| Compare luna against PUC/LuaJIT | [`performance.md`](performance.md) |
| Run luna in an async runtime | [`threading.md`](threading.md) |
| Hack on the JIT | `crates/luna-jit/src/jit_backend/` |
| Hack on the interpreter | `crates/luna-core/src/vm/exec.rs` |
| Hack on the GC | `crates/luna-core/src/runtime/gc.rs` |
| Hack on a stdlib library | `crates/luna-core/src/vm/lib_*.rs` |
