# Changelog

All notable changes to luna will be documented in this file. Format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The public stability contract for the 1.x line covers:

- `pub` items in `src/lib.rs`'s exported tree
  (`luna::vm::Vm`, `luna::runtime::Value`, `luna::version::LuaVersion`,
  `luna::frontend::*` parser surface)
- The `lua.h`-compatible C ABI under `src/capi.rs`
- Bytecode binary compatibility with PUC Lua per-dialect (`.luac`
  files load in and out)

Internal modules (JIT codegen, dispatcher hot-path internals, heap
internals) may change without notice within 1.x for performance
optimization.

---

## [1.2.0] — 2026-06-24

Polish + ergonomics sprint on the v1.1 ship. Headline: **`LuaUserdata`
trait sugar** for Lua-callable host types, REPL gets multi-line input
plus history, lint debt cleared, perf attack discovers the real
bottleneck (interp, not trace) and updates the methodology accordingly.

### Track B — `LuaUserdata` trait (new embedder surface)

- **`luna_core::vm::userdata_trait`** module exposes the
  [`LuaUserdata`](https://docs.rs/luna-core/1.2/luna_core/vm/trait.LuaUserdata.html)
  trait + [`UserdataMethods<T>`](https://docs.rs/luna-core/1.2/luna_core/vm/trait.UserdataMethods.html)
  builder + [`MetaMethod`](https://docs.rs/luna-core/1.2/luna_core/vm/enum.MetaMethod.html)
  enum. Embedders register methods (`add_method` / `add_method_mut`),
  static fns (`add_function`), metamethods (`add_meta_method`), and
  call-syntax field getters (`add_field_method_get`) via a typed
  builder.
- **Per-Vm metatable cache** keyed by `TypeId::of::<T>()`. First
  `create_userdata::<T>` triggers `T::add_methods` once; subsequent
  instances reuse the cached `Gc<Table>`. Pinned via `pin_host` so
  GC keeps the metatable live.
- **`Vm::create_userdata` / `Vm::set_userdata` bound tightened** from
  `T: Any + 'static` to `T: LuaUserdata`. **BREAKING**: existing
  B8 users upgrade with `impl LuaUserdata for MyType {}` (one line).
- **Auto-install metatable + `__gc` finalizer wire** at userdata
  allocation time (`check_finalizer_userdata` called from
  `create_userdata`).
- **`FromLuaArgs::from_lua_args_skip_self`** added — the
  method-call shape where slot 0 is the receiver.
- **`FromLuaArgs for Vec<Value>`** — variadic decoder for
  dispatcher-style natives (e.g. `redis:call(cmd, ...)`).
- Three new runnable examples:
  `examples/userdata_demo.rs` (Counter), `userdata_vec3.rs`
  (arithmetic metamethods), `userdata_redis_stub.rs` (dogfood §4.1
  shape — state IS the payload, no `thread_local!`).
- `docs/embedding.md` §7 rewritten with subsections covering trait
  shape, static constructors, variadic dispatch, the v1.2 field-style
  limitation (call-syntax only — true `obj.x` deferred to v1.3, see
  Deferred section), GC ordering, and trait contract reminders.

### Track R — REPL

- **Multi-line continuation**: incomplete statements (detected via
  `SyntaxError::msg.contains(" near <eof>")`) emit `>>` and accept
  another line. `local x = function()` + `return 1` + `end` now
  works at the REPL instead of erroring on line 1.
- **`~/.luna_history` persistence**: 1000-entry capped history,
  loaded on startup, saved on exit. No new dependency
  (`std::env::var_os("HOME")` only).

### Track L — Lint debt cleared

- `cargo fmt --all` clean (cleared 606-site formatter drift from v1.0/v1.1).
- `cargo clippy --workspace --all-targets -- -D warnings` clean
  (12 historic errors fixed: 8 `not_unsafe_ptr_arg_deref` justified
  with rationale, 2 `approx_constant` → `std::f64::consts::PI`,
  1 ZST `uninit_assumed_init` constant-folded guard, 2 dialect-test
  fixture allows).
- `cargo fix` unused-imports sweep across 60+ files plus 5 hand-fixed
  clippy issues (unnecessary unsafe, match→unwrap_or_default, etc.).
- Workspace `[lints.clippy]` policy in `Cargo.toml` declares the
  strict baseline and the few documented exemptions
  (`missing_safety_doc` — `docs/unsafe-accounting.md` is SoT;
  `incompatible_msrv`, `too_many_arguments`, etc.).

### Track P — Perf attack (real bottleneck identified)

- **D2 criterion infra** + Linux CI runner workflow_dispatch
  perf-gate (manual trigger; `redis_lua_shape` baseline).
- **D3 TA1 Path B lowerer**: `GetTabUp` admitted into the trace
  recorder as a standalone helper (was: unconditional bail at
  `trace.rs:3030`). Traces compile end-to-end on the token-bucket
  shape; bail rate 0.
- **D4 A1 GetField fast path**: `Table::get_str` + Op::GetField
  interp arm skip `op_index` when the receiver is a known `Value::Str`
  with no metatable (commit `a2c98ae`).
- **`Vm::current_op`** API (ergo.rs) + `diag_opcode_breakdown.rs`
  example — runtime opcode counter for `[perf-decomposition-vs-polish.md]`
  §2 Phase A "actual workload validates the decomp" hard gate.
- **Methodology lesson** (`docs/performance.md` + global
  methodology doc updated): the v1.0 charter hypothesis "1.5×
  gap vs PUC 5.1 on token_bucket" was 4× understated. PUC 5.5 is
  ~4.1× faster than luna interp on the shape; LuaJIT 2.1 is ~196×.
  **True attack surface = interp,  not trace.** Trace JIT does not
  engage on the Redis-Lua-shape workload (`infer_getx_exit` returns
  None on the `Call(Native math.min)` mid-body; length-gate kicks in
  on short bodies). D4 A3/A4/A5 + Path B math-fold extend recorded
  as Deferred-to-v1.3 (NOT silent — see Deferred).

### Track S — `feature = "send"` framework reserved

- `[features] send = []` declared in `crates/luna-core/Cargo.toml`.
  Building with `--features send` triggers `compile_error!` pointing
  to `v1.3-rfc-send-arc.md`. Embedders can feature-detect (`cargo
  add luna-core --features send` fails loudly) without waiting for
  the v1.3 implementation.
- Phase 0 audit (`v1.2-audit-send-cost.md`): ARM M-series ~10%
  overhead (within RFC 15% ceiling); x86_64 Linux ~20% (refines the
  RFC ceiling, needs `SendVm` newtype fork in v1.3).

### CI / release infra (Track G)

- **Lint gate**: `cargo fmt --check` + `cargo clippy --workspace
  --all-targets -- -D warnings` on every push.
- **0-dep gate**: `cargo tree -p luna-core --prefix none` must show
  exactly one line (luna-core itself). Catches accidental
  dependency creep at PR time.
- **Unsafe-drift gate** (new in v1.2): first-party unsafe site count
  must stay under a recorded ceiling (490, baseline 461 from v1.1
  + ~15 from Track B). Bump the ceiling explicitly when justified;
  never widen to silence drift.
- `branches: [main]` → `[master, develop]` to track git-flow setup.
- `docs/release-checklist.md` (new) — version-agnostic checklist
  template; sprint-specific audits stay under `.dev/`.
- `.dev/discussions/luna-crate-name-history.md` — archives the
  v1.1.0 ship-time rename story (`luna` → `luna-jit`).

### Deferred to v1.3 (NOT silent)

These items are scoped out of v1.2 explicitly:

- **Path B math-fold extend** (`min` / `max` 2-arg) — required for
  trace JIT to actually dispatch on token-bucket-style workloads.
  Audit ~1-2d effort. Bundled with TA3 `trace_enabled` default flip
  + Linux taskset bench (macOS local variance band is too wide).
- **D4 A3 / A4 / A5** (newindex double-walk collapse / Move
  elimination / dispatcher reshape) — audit revealed cost estimates
  4-5× too high; marginal once A1 lands.
- **`add_field_method_set` + true `obj.x` field-style access** —
  v1.2 trait sugar ships call-syntax only (`obj:width()`). True
  field-style needs `__index` as a function dispatcher.
- **`#[derive(LuaUserdata)]` proc-macro** — hand impl is mlua's
  surface; revisit if dogfood reports demand the derive.
- **Track S `feature="send"` actual impl** — 1Q sprint scoped in
  `v1.2-audit-send-cost.md`; needs `SendVm` newtype fork if Phase A
  bench confirms x86_64 budget overrun.
- **REPL C3 tab completion + syntax highlight** — gated on adding
  `rustyline` as a non-default `repl-line-editor` cargo feature.
- **PUC 5.4 luac body loading (Track E E1)** — Q1 conditional on
  ship-time budget; if not v1.2, lands in a v1.3 quarter sprint
  covering 5.1-5.5 binary formats together.

### Internal — sprint methodology

- `.dev/perf-baselines/2026-06-24-*.md` records the decomp work
  that surfaced "interp not trace" as the true attack surface.
- `~/.claude-shared/global/methodology/perf-decomposition-vs-polish.md`
  gained the v2-* polish-disaster anti-pattern catalog from the
  v1.0 fib_28 misdirection.
- Charter, plan-state, and audit docs live in `.dev/rfcs/v1.2-*.md`
  (gitignored); `docs/` stays user-facing.

---

## [1.1.0] — 2026-06-23

### Ship-time crate rename

The JIT-equipped crate is published as **`luna-jit`** instead of
`luna` because the `luna` name on crates.io is taken by an
unrelated utilities library. The directory layout, library
exports, and CLI binary name (`luna`) are unchanged; only the
crate name visible on crates.io is `luna-jit`. Embedders use:

```toml
[dependencies]
luna-jit = "1.1"   # or:   luna-core = "1.1"   for the 0-dep core
```

```rust
use luna_jit::Lua;   // (was `use luna::Lua;`)
```

The CLI binary still installs as `luna` (`cargo install luna-jit`
puts a binary named `luna` on PATH). `luna-core` keeps its name
(0-dep interpreter is the pure thing).

### Track A — Crate / Dep / Safety

- **Workspace split** (A1): `luna-core` (0 third-party deps; lexer /
  parser / compiler / interpreter / runtime / stdlib / GC / pattern /
  JIT trait surface) and `luna` (Cranelift JIT + capi + CLI binary).
  `cargo add luna-core` pulls only the interpreter; `cargo add luna`
  pulls the full JIT'd stack. CI gate: `cargo tree -p luna-core`
  must show exactly one crate.
- **JIT trait boundary** (A1 Session A): `IntChunkCompiler` /
  `TraceCompiler` traits in `luna_core::jit::abi` decouple the
  dispatcher from Cranelift. `NullJitBackend` (in `luna-core`) and
  `CraneliftBackend` (in `luna`) implement the traits.
- **`Vm::new_minimal_with_jit`** in the `luna` crate — one-line
  constructor for embedders wanting the v1.0 JIT-on-by-default
  behavior through `cargo add luna`.
- **`Vm` rustdoc + `!Send` compile_fail doctest** (A7) — `Vm: !Send + !Sync`
  is now CI-enforced. `docs/threading.md` covers canonical
  embedding patterns.
- **`JitState` sidecar** (A2): JIT-specific Vm fields factored into
  a dedicated struct, freeing the Vm hot path from JIT churn.
- **SAFETY: comment coverage** (A6): 100% across `unsafe { ... }`
  blocks. 342 new annotations added. See `docs/unsafe-accounting.md`.
- **Public API 0 unsafe** (A4): 4 `pub unsafe fn` items demoted to
  `#[doc(hidden)]`; `TableBuilder` / `IntoValue` / `native_typed`
  cover the safe embedder flows. The dogfood §4.1 friction is closed.
- **Panic-safe public boundaries** (A5): `Vm::set_global` returns
  `Result<(), LuaError>`; 68 call sites updated.
- **`cargo-deny`** (A3): CI workflow gates supply chain (advisories,
  licenses, source registry) plus a hard `luna-core` 0-dep check.

### Track B — Embedder API

- **`Vm::sandbox(version).build()`** (B1): Conservative-default
  sandbox builder; embedders whitelist stdlib modules + set
  instr/memory budgets in one chain.
- **`vm.eval` / `vm.eval_chunk`** (B2): Single-call source-to-value
  evaluation returning `Result<Vec<Value>, LuaError>`. SyntaxError
  surfaces as a heap-interned `LuaError`.
- **`TableBuilder` + `vm.table_of`** (B3): Build tables with chained
  `.with(k, v)` calls or a fixed-size slice. Embedders never write
  `unsafe { gc.as_mut() }` for table construction.
- **`IntoValue` trait** (B4): `vm.set_global("k", 42_i64)` infers;
  blanket impls cover `i64`, `f64`, `bool`, `&str`, `String`,
  `Vec<u8>`, `Gc<Table>`, `Gc<LuaClosure>`, `Gc<NativeClosure>`,
  `Value`, `()`, `Option<T>`.
- **`vm.native_typed` + `FromLuaArgs`/`IntoLuaReturn`/`FromLuaValue`**
  (B5): Typed Rust functions exposed as Lua callables. Arities 0-6,
  fn pointers and non-capturing closures, multi-value returns,
  `Result<T, LuaError>` for fallible natives.
- **Structured `LuaError`** (B6): Adds `LuaErrorKind` enum
  (Runtime / Syntax / InstrBudget / MemoryCap / Native /
  OutOfMemory / Type), `impl Display + Error` on `LuaError`,
  Vm-side `error_kind` / `error_source` / `take_error_traceback`
  accessors. `LuaError` stays `Copy`.
- **String interop** (B7): `vm.intern_str`, `Value::try_as_str`
  (UTF-8 validating), `Value::as_bytes` (binary-safe).
- **Host userdata** (B8): `vm.create_userdata::<T>(value)` /
  `set_userdata` / `userdata_borrow` / `Userdata::downcast` for
  arbitrary `T: 'static` host types. The closed-world userdata
  infrastructure now accepts host payloads.
- **Rust-side coroutine drive** (B9): `vm.create_coroutine` /
  `vm.resume_coroutine` parallel to `coroutine.create` / `:resume`.
- **Async embedder API** (B10): `vm.eval_async` returns a `!Send`
  Future driving the dispatcher with cooperative yields on
  instruction budget exhaustion. `vm.set_async_native` exposes
  async Rust functions to Lua scripts. `Lua::eval_async` /
  `Lua::set_async_native` mirror on the facade.
  `examples/async_host.rs` ships a runnable Tokio-substitute
  walkthrough. 0 new third-party deps (`std::future` + `std::task`
  suffice).
- **Rust-side debug hook** (B11): `vm.set_rust_debug_hook` accepts
  a `fn(&mut Vm, RustHookEvent)` plus mask flags
  (HOOK_MASK_CALL / RETURN / LINE / COUNT). Both Lua-side
  `debug.sethook` and Rust hooks can coexist.
- **`Lua` newtype facade** (B12): `mlua`-shape front door with
  owned `LuaFunction` / `LuaTable` / `LuaRoot` handles backed by
  an append-only `Vm::host_roots` pool. Use `Lua::new()` for the
  five-minute start; use `Vm` for the low-level handle.

### Track C — CLI / REPL

- **Interactive REPL** (C1): `luna` with no args drops into a
  single-line REPL. Each line is tried as an expression
  (`return <line>`), then as a statement on syntax error.
- **CLI flags** (C4): `--sandbox` builds via SandboxBuilder;
  `--budget=N` sets instr budget; `--no-jit` installs NullJitBackend;
  `--profile` prints trace-JIT counters on exit.
- **Pretty errors** (C5): Compile + runtime errors render with
  classified kind tag, source location, snippet, and traceback.
  ANSI color when stderr is a TTY and `NO_COLOR` is unset.

### Track D — Bench / Perf

- **Redis-Lua-shape micro-bench** (D1): New `redis_lua_shape` bench
  with four workload shapes from the dogfood report
  (`token_bucket_1k`, `sliding_window_500`, `method_dispatch_5k`,
  `string_ops_2k`).
- **`docs/performance.md` extension** (F4): D1 baseline added
  alongside the cross-dialect snapshot.

### Track E — Dialect / require / Compat

- **`docs/compatibility.md` extension** (E2): v1.1 luna-specific
  extension table + CLI options reference + REPL behavior.

### Track F — Docs

- `docs/architecture.md` (F5): crate layout + source classification
  + JIT pipeline + threading + sandbox.
- `docs/threading.md` (A7 artifact): `!Send` patterns + Tokio +
  async embedder API.
- `docs/embedding.md` (F1): 12-section embedder cookbook
  (install / hello / sandbox / globals / tables / native_typed /
  userdata / coroutines / debug hooks / errors / Lua facade /
  threading).
- `docs/binary-size.md` (G5): cargo-bloat snapshot
  (cranelift_codegen 45% / luna_core 25% / std 13%).
- `docs/unsafe-accounting.md` (G4): cargo-geiger companion;
  461 unsafe sites, 394 SAFETY-annotated, 6 pattern categories.
- README.md rewrite (F6): workspace + ergo + honest perf.

### Track G — CI / Release

- **MSRV declaration** (G1): `rust-version = "1.86"` in
  `[workspace.package]`; CI workflow `.github/workflows/msrv.yml`
  locks against it.
- **CI matrix** (G2): `.github/workflows/ci.yml` runs
  build/test/release/doc on Linux + macOS + Windows + wasm32
  (luna-core only). `cargo doc --workspace -D warnings` gate.
- **`cargo-deny`** (A3, listed above): supply-chain + 0-dep gate.

### Changed

- Source tree reorganization: `src/jit/trace.rs` (9483 LOC) split
  in place into `trace.rs` (Cranelift codegen body) and
  `trace_types.rs` (type definitions + thresholds + cranelift-free
  helpers). Type paths preserved via re-exports; downstream
  callers see no API change.
- `Vm::set_global` signature changed from
  `(&mut self, name: &str, v: Value)` to
  `<V: IntoValue>(&mut self, name: &str, v: V) -> Result<(), LuaError>`.
  Existing callers passing `Value::*` directly still compile (V
  infers to Value). New ergonomics: `vm.set_global("k", 42)`.

### Deferred to v1.2

- C2 (REPL multi-line continuation + history)
- C3 (REPL tab completion + syntax highlight, likely as
  `luna-repl` binary crate)
- D2 (criterion infra + n=1000 + CPU pin + 10 runs)
- D3 (token_bucket decomposition vs PUC 5.1)
- D4 (attack-agent perf workflow)
- E1 (require searcher table dispatch — behavior change requires
  PUC test re-verification)
- E3 (PUC `luac` body 5.1-5.5 compat — 20-30 day block, charter L)
- E4 (string.pack/utf8 edge case test gaps)
- Lint cleanup (`cargo fmt --all` 606 sites + 9 `clippy` errors,
  see `.dev/known-bugs/historic-fmt-clippy-drift.md`)
- `feature = "send"` `Arc<RwLock<T>>` sprint (see
  `.dev/rfcs/v1.1-rfc-vm-send-sync.md`)
- `LuaUserdata` trait sugar (B8 follow-on; closed-world ships
  v1.1, trait sugar lands later)

---

## [1.0.0] — 2026-06-23

First stable release. luna implements **Lua 5.1, 5.2, 5.3, 5.4, and
5.5** in pure Rust with zero non-build dependencies (cranelift is
the JIT codegen).

### Correctness

- **910 tests / 0 failures / 0 ignored**
  - 242 lib unit
  - 123 PUC official-suite files across 5 dialects (5.1 = 23,
    5.2 = 26, 5.3 = 27, 5.4 = 32, 5.5 = 15)
  - 40 end-to-end programs × 5 dialects byte-diff vs installed PUC
    binary
  - 64 method-JIT dialect-audit tests (`Value`-variant introspection)
  - 28 trace-JIT audit tests
  - 13 C API conformance tests
  - 10 sandbox embedding tests
  - 8 fast smoke tests
  - ~500 trace-JIT integration tests

### Performance

Master gate (`vs.X ≤ 0.50`, luna ≥ 2× the reference):

- **vs PUC 5.1-5.5: 35 / 35 cells PASS** across all 7 microbench
  workloads × 5 dialects
- **vs LuaJIT 2.1: 6 / 7 cells PASS**. `binary_trees_n10` lands at
  0.83× (luna 1.21× faster than LuaJIT 2.1, just shy of the 2× gate)
  — this is the design ceiling under luna's no-NaN-boxing + PUC
  bytecode-compat constraints.

See `docs/performance.md` for the full snapshot.

### Public surface (frozen for 1.x)

- Rust embedding API: `Vm`, `Value`, `LuaVersion`, the `Vm::open_*()`
  stdlib loaders, the native-function registration helpers
- Script-host sandbox pattern: see `examples/sandbox_demo.rs` and
  `tests/sandbox.rs`
- C ABI: `lua.h`-compatible subset under `src/capi.rs`, conformance
  locked by `tests/capi.rs`
- Bytecode binary compat: PUC-compiled `.luac` files load directly
  into luna for the corresponding dialect; luna's compiler emits
  matching format

### Major features

- Full dialect support — all 5 Lua versions in a single binary,
  per-`Vm` dialect selection
- Cranelift method-JIT for hot top-level chunks + cranelift
  trace-JIT for hot loop / recursive shapes
- PUC-faithful Lua semantics including: integer subtype (5.3+),
  bitwise operators (5.3+), `<const>` / `<close>` attributes (5.4+),
  `global` keyword + named varargs (5.5+), `goto` / labels (5.2+),
  full coroutine + metatable + weak-table + `__gc` finalizer
  support, generational GC pacing
- Sandbox-grade embedding: per-`Vm` instruction + memory budgets,
  bytecode-load gating, host native callbacks, no required global
  state

### Documentation

- `README.md` — overview + quick-start
- `docs/compatibility.md` — embedder compatibility surface
- `docs/performance.md` — perf snapshot
- `cargo doc --open` — full API reference

### Test environment

Tested on macOS 25.5 / aarch64 (M-series) with rustc 1.86+ and
cranelift 0.124. PUC binaries: Lua 5.1.5, 5.2.4, 5.3.6 built from
source; Lua 5.4.8, 5.5.0 + LuaJIT 2.1.1781602682 via brew.
