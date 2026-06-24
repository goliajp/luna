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

## [1.3.0] — TBD

**Mega sprint** — 2026-06-24 user directive collapsed the planned
v1.2.0 + v1.3.0 + v1.4.0 + parts of v2.0 into a single ship under
the `nodefer` upgrade ("nothing is deferred to v1.4 or later").
Headline phases:

- **Phase A** (was v1.2): `LuaUserdata` trait sugar, REPL multi-line
  + history, lint debt cleared, Track B/L/P/R/S/G floor — already
  on develop (commits `bc088bd` / `65ca2cc` / `70c4bff`).
- **Phase B-N** (v1.3 expanded): PUC luac body 5.1-5.5, Send-safety
  full impl, perf attack round 2 (Path B math-fold extend), wasm32-
  wasip1 port, true `obj.x` field-style + `derive(LuaUserdata)`,
  REPL tab + syntax highlight, async natives in dispatch, userdata
  Trace-bearing host payloads, host_roots slot recycling, **luna-aot
  native-binary compile**, **MacroLua dialect support**.

See `.dev/rfcs/v1.3-charter.md` for the full track list, time
window estimate, and Phase ordering. `nodefer` is the operating
contract: every line item ships in v1.3 or is documented as
permanently out-of-scope (currently only the `luna` crates.io name
reclaim falls there — sticking with `luna-jit`).

The Phase A content below was previously written under the
`[1.2.0]` heading; it ships now as part of v1.3.0 without a
separately-published v1.2.0 on crates.io.

### Phase A headline

Polish + ergonomics on the v1.1 ship. **`LuaUserdata`
trait sugar** for Lua-callable host types, REPL gets multi-line input
plus history, lint debt cleared, perf attack discovers the real
bottleneck (interp, not trace) and updates the methodology accordingly.

### Track B — `LuaUserdata` trait (new embedder surface)

- **`luna_core::vm::userdata_trait`** module exposes the
  [`LuaUserdata`](https://docs.rs/luna-core/1.3/luna_core/vm/trait.LuaUserdata.html)
  trait + [`UserdataMethods<T>`](https://docs.rs/luna-core/1.3/luna_core/vm/trait.UserdataMethods.html)
  builder + [`MetaMethod`](https://docs.rs/luna-core/1.3/luna_core/vm/enum.MetaMethod.html)
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

### Phase B-N — v1.3 expansion in flight

Per the 2026-06-24 `nodefer` directive every item below is **in
scope** for v1.3 (no longer deferred). Tracked in
`.dev/rfcs/v1.3-charter.md` + `.dev/rfcs/v1.3-plan-state.md`:

- **Path B math-fold extend** (`min` / `max` 2-arg) — *(landed Phase P2A)*
  `trace.rs::try_match_trace_math_fold` extended with `FoldKind::Min2 /
  Max2`. Split-window recognizer (only `GetTabUp + GetField + Call`
  flagged in `folded_ops` — arg-prep ops execute normally). Cranelift
  `smin/smax` for Int/Int, `fmin/fmax` for Float-or-mixed.
  `trace_dispatched_count` flipped 0 → 200/200 on `diag_token_bucket`.
  **TA3 default flip done** — `jit_state.rs::with_null_backend` ships
  `trace_enabled = true` (was `false`) after Linux taskset perf-gate
  confirmed `redis_lua_shape ≥ 1.0×` v1.2 baseline. Embedders that want
  the v1.2 interp-only default call `vm.set_trace_jit_enabled(false)`.
- **D4 A3 / A4 / A5** (newindex double-walk collapse / Move
  elimination / dispatcher reshape) — perf polish on top of A1.
- **`add_field_method_set` + true `obj.x` field-style access** —
  *(Phase UD1+UD2 landed)* `add_field_method_set(name, fn)` registers
  setters for `obj.name = value`; the `__index` slot becomes a native
  trampoline when any getter is registered, so `obj.width` (no parens)
  resolves to the field value directly. **Breaking change** from the
  v1.2 sugar: `obj:name()` call-syntax for `add_field_method_get` no
  longer works (the trampoline calls the getter and returns
  `Int(...)`, so `Int(...)(obj)` errors). Embedders who need both
  shapes should register an explicit `add_method("name", ...)`
  alongside the field-getter. Unknown writes go to a runtime error
  rather than silently dropping (`code/no-unsolicited-fallback`).
- **`#[derive(LuaUserdata)]` proc-macro** — *(Phase UD3 landed)* new
  `luna-jit-derive` crate ships the derive + `#[lua_userdata_methods]`
  attr macro. Helper attributes: `#[lua_method("name")]`,
  `#[lua_method_mut]`, `#[lua_function]`, `#[lua_meta_method(Add)]`,
  `#[lua_meta_method_mut]`, `#[lua_field_get]`, `#[lua_field_set]`,
  `#[lua_skip]`, plus struct-level `#[lua_type_name = "X"]`. Hand
  impl stays as the escape hatch (generic types, conditional method
  sets). luna-core 0-dep contract preserved — derive lives in
  `luna-jit-derive` only; luna-jit's build-time supply chain grows by
  `syn + quote + proc-macro2` (the standard derive trio). `cargo
  tree -p luna-core --prefix none --no-default-features` still
  reports 1 row. Embedders writing `use luna_jit::LuaUserdata;` get
  both the trait (via the `pub use luna_core::*;` re-export) and the
  derive (`pub use luna_jit_derive::LuaUserdata;`).
- **`feature = "send"` real implementation** *(Phase SS-B landed)*
  — new opt-in cargo feature on luna-core (`send = []`) and
  luna-jit (`send = ["luna-core/send"]`) surfaces a second public
  type `luna_core::vm::SendVm` for cross-thread embedding. Shape:
  `SendVm { inner: Arc<UnsafeCell<Vm>>, lock: Arc<RwLock<()>> }`
  with `unsafe impl Send for SendVm` (justified by a runtime
  single-mutator invariant the lock re-establishes). Default-feature
  builds are bit-identical with the pre-SS-B baseline — bare `Vm`
  stays `!Send + !Sync` and pays no overhead. luna-core 0-dep
  contract preserved (`Arc`, `UnsafeCell`, `RwLock` are all
  stdlib).
  - **API surface mirror**: `eval`, `call_value`, `set_global`,
    `set_userdata`, `intern_str`, `open_base / open_math /
    open_string / open_table / open_coroutine`, the Phase SR
    `pin_host / read_host / unpin` host-roots methods, plus
    `Clone` (cheap — two `Arc::clone`), `Debug`, and one new
    method `get_global(name) -> Value` that isn't present on bare
    `Vm` (introduced because the bare `globals()` + raw `Gc<Table>`
    deref is awkward across the lock boundary).
  - **Interp-only constraint**: `SendVm::new` calls
    `Vm::new_minimal` which leaves `JitState` at `NullJitBackend`.
    The trace JIT does not run on a SendVm in v1.3. JIT-aware
    SendVm is a documented post-v1.3 polish item (the
    `Proto::traces: RefCell<Vec<Rc<CompiledTrace>>>` field
    intersects with `Send` and would need an `Rc → Arc` migration;
    audit projects ~6 % additional JIT-engaged cost). Not a
    defer — the v1.3 charter explicitly scopes interp-only as the
    SS-B deliverable.
  - **Cost** (macOS M-series, SS-B bench): SendVm pays ~+1.86 %
    token-bucket regression vs interp-only baseline `Vm` (175.46
    µs vs 172.26 µs). Better than the audit's projected ~3 % ARM.
    Linux x86_64 numbers land via the `perf-gate` CI matrix
    (audit projects ~6 %).
  - **8 smoke tests** in `crates/luna-core/tests/send_vm.rs`
    (gated `#[cfg(feature = "send")]`): compile-time `Send`
    assertion, basic eval, `thread::spawn` move, 100-thread
    concurrent contention (verifies serialized counter = 4950),
    userdata round-trip, HostRootTicket round-trip across the
    lock, pin-across-clones, and interp-only loop sum.
  - **Bench update** (`crates/luna-jit/benches/bench_send_overhead.rs`):
    feature-gated `send_vm_eval` and `send_vm_token_bucket` pairs
    added alongside the SS-A `wrapped_vm_*` NoOpWrapper baseline;
    apples-to-apples interp-bare counterparts (`bare_vm_interp_*`)
    added for the SendVm comparison.
  - **Documentation**: `docs/threading.md` gains a `SendVm`
    section covering when to use vs not, the shape + soundness
    story, the interp-only constraint, and a tokio multi_thread
    embed example (without depending on tokio in luna-core).
  - **Design RFC**: `.dev/rfcs/v1.3-rfc-send-arc.md` documents
    the as-shipped wrapper choice + the decision to defer the
    audit's per-field `SendGc<T>` fork to v1.4+.
  - **Unsafe drift**: +5 first-party `unsafe` sites (480 → 485,
    ceiling 490 — 5 slots free). New sites: `unsafe impl Send for
    SendVm` (one), `&mut *UnsafeCell::get()` inside
    `with_vm_mut` (one), `(*globals.as_ptr()).get(key)` in
    `get_global` (one), two doc-comment occurrences caught by the
    grep regex.
  - **BREAKING vs v1.2 stub**: the v1.2 `[features] send = []`
    that raised a `compile_error!` when selected now compiles
    cleanly and surfaces `SendVm`. Embedders who were guarding
    against the compile_error with `cfg(not(feature = "send"))`
    no longer need that guard.
- **REPL C3 tab completion + syntax highlight** — `[features]
  repl-line-editor` (rustyline) non-default cargo feature.
- **PUC luac body 5.1-5.5** — full binary compat across all
  shipping Lua dialects; opt-in `Vm::set_puc_bytecode_loading(true)`
  + per-dialect translator under `crates/luna-core/src/vm/dump/`.
- **wasm32-wasip1 support** — `io.popen` / `os.execute` cfg-gated
  + wasi stubs return PUC error tuple.
- **`official_run` flakiness fix** — compiler short-circuit AND
  `debug_assert_eq!(reg, base)` + sweep misaligned-pointer cascade
  root cause + fix.
- **Async natives in dispatcher** (B11 hook firing) *(Phase AS
  landed)* — close the v1.1 B10 Stage 2 deferred path so async-marked
  natives compose with Rust-side `[B11]` debug hooks. Audit
  (`.dev/rfcs/v1.3-audit-async-natives.md`) showed the gap was
  narrower than the v1.1 charter assumed: the dispatcher hot loop's
  `Count` / `Line` / Lua-`Call` / Lua-`Return` sites are opcode-driven
  and already fire correctly under `async_mode = true`; only the
  async-native call boundary itself was missing. Phase AS adds:
  - `Call` event on the async-native branch in
    `crates/luna-core/src/vm/exec.rs`, fired after the
    `native_nresults` / `gc_top` pin and before the future is built —
    same placement-relative-to-pin as the sync native path's
    `hook_call(true, nargs)` site (audit §A.1 / Q6).
  - `Return` event in `Vm::commit_async_native_result`
    (`crates/luna-core/src/vm/async_drive.rs`), fired after
    `finish_results` lands the resolved nret into the call window and
    before the post-call GC checkpoint. Mirrors the sync native's
    `hook_return(true, nargs + 1, nret)` placement. The method is
    now fallible — `EvalFuture::poll`'s `Poll::Ready(Ok(nret))` arm
    propagates the hook error through the same JIT-restore + cleanup
    path the `Poll::Ready(Err)` arm already runs.
  - **Count + Line carryover** — no code change; the dispatcher's
    persistent `hook.count_left` and `hook_lastline` `Vm` fields
    already carry across `Poll::Pending` returns to the executor, so
    a 1000-instruction count budget walks down naturally across
    arbitrarily many slice boundaries and a line event won't
    double-fire on resume mid-line. New tests pin both as regression
    guards.
  - **6 smoke tests** in
    `crates/luna-core/tests/async_hook_composition.rs`: `Call`/`Return`
    around an immediate-Ready async native, `Call`/`Return` bracketing
    a yield-once async native (proves the Return fires after `.await`
    resolves), count-hook carryover across an aggressive 50-op slice,
    line-hook dedupe across a 3-op slice, compile-time
    `assert_send::<RustDebugHook>()` + `assert_sync::<RustDebugHook>()`
    pinning the function-pointer Send-safety property, and a
    composition smoke confirming the hook body observes the
    async-native Call event end-to-end. No tokio dep — same
    hand-rolled `block_on` + `YieldOnce` harness as the existing
    `tests/async_native.rs` (luna-core 0-third-party-dep contract
    preserved).
  - **`Send` composition with SS-B** — `RustDebugHook = fn(&mut Vm,
    RustHookEvent)` is a bare function pointer and unconditionally
    `Send + Sync`, so the v1.3 Phase SS-B `SendVm` newtype composes
    cleanly with async hooks without any new trait bound. The
    compile-time `assert_send` test is the regression guard for any
    future evolution of the hook signature toward closure state.
  - **Re-entrancy contract**: hook bodies under async mode may call
    sync `vm.eval(...)` but must NOT invoke async natives — the
    inner sync `eval` lacks an executor to drive a nested
    `EvalFuture`, and the existing rejection
    (`"async native called in sync context"`) catches the attempt
    cleanly. Documented in `docs/threading.md` §"Async natives +
    debug hooks".
  - **Q5 followup** (audit §"Open questions"): `EvalFuture::Drop`
    already clears `pending_async_native_fut` /
    `pending_async_native_ctx` (`async_drive.rs:553-554` in the
    pre-AS code), so the stale-ctx hardening the audit flagged is
    already in place — no additional cleanup required in Phase AS.
  - **Unsafe drift**: 0 new sites. Hook visibility bump from `fn`
    to `pub(crate) fn` on `Vm::hook_call` and `Vm::hook_return` is
    safe-Rust-only.
- **Userdata `Trace`-bearing host payloads** — `T` may hold
  `Gc<...>` fields; collector recurses into the payload (userdata
  GC ripple).
- **`host_roots` slot recycling** *(Phase SR landed)* — the v1.1
  append-only `Vec<Value>` is replaced by a free-list-backed slot
  pool keyed by `HostRootTicket { idx: u32, generation: u32 }`
  (8 bytes, `Copy`). `pin_host` returns the ticket; `unpin` clears
  the slot to `Nil`, bumps generation, and pushes the index onto
  the free list for reuse; `read_host` / `write_host` validate the
  ticket's generation and return `None` / `Err(HostRootStale)` on
  stale lookup (ABA-safe). Generation overflow at `u32::MAX` retires
  the slot permanently (bounded leak: ~4 days at 10⁹ unpins/day per
  slot). Long-running embedders (request-per-script loops, edge
  workers) now hold at a bounded pool size instead of growing the
  vector monotonically.
  - **BREAKING — embedder Vm API**: `Vm::pin_host(v: Value) -> usize`
    is now `Vm::pin_host(v: Value) -> HostRootTicket`;
    `Vm::host_root_at(idx) -> Value` and `Vm::host_root_set(idx, v)`
    are **removed** in favor of `Vm::read_host(t) -> Option<Value>`
    and `Vm::write_host(t, v) -> Result<(), HostRootStale>`. New
    methods: `Vm::unpin(t) -> Result<(), HostRootStale>`. Existing
    `Vm::unpin_all()` and `Vm::host_root_count() -> usize` signatures
    unchanged; `unpin_all` semantics extended to bump every slot's
    generation (all outstanding tickets become stale uniformly).
    Migration: replace stored `usize` index with `HostRootTicket`;
    `vm.host_root_at(idx)` → `vm.read_host(ticket).expect("...")`;
    `vm.host_root_set(idx, v)` → `vm.write_host(ticket, v).unwrap()`.
  - **BREAKING — `luna-jit` facade structs**: `LuaFunction` /
    `LuaTable` / `LuaRoot` now carry `ticket: HostRootTicket`
    (was `idx: usize`). `Copy + Clone` preserved; public method
    surface (`call` / `call_multi` / `get` / `set` / etc.) is
    invariant. New `Lua::unpin(handle)` releases a single handle
    via the new `PinnedHandle` trait (impl'd by all three handle
    types). Reads after `Lua::unpin` / `Lua::unpin_all` panic with
    `"<HandleType> used after unpin / unpin_all"` — matches the v1.1
    "handles created before `unpin_all` become invalid" docstring.
  - New module: `luna_core::vm::host_roots` (own the pool impls);
    types re-exported as `luna_core::vm::{HostRootTicket, HostRootStale}`.
    Tests: `crates/luna-core/tests/host_roots_slot_recycling.rs`
    (10 tests covering basic recycle, ABA detection, `unpin_all`
    invalidation, 100k pin/unpin smoke, free-list LIFO, GC tracer
    correctness across recycle).
- **`luna-aot` native-binary compile** *(Phase AOT scaffold landed;
  Cranelift codegen follow-up within v1.3)* — new sibling crate
  `crates/luna-aot/` (workspace member alongside `luna-core` +
  `luna-jit` + `luna-jit-derive`). Ahead-of-time compiler that
  emits a self-contained binary embedding the Lua bytecode with no
  runtime parse step.
  - **Scaffold pipeline end-to-end** today: Lua source →
    `luna_core::frontend::parser::parse` → AST →
    `luna_core::compiler::compile_chunk` → `Gc<Proto>` →
    `luna_core::vm::dump::dump` → luna body dump bytes →
    `object::write::Object` with a `.luna.bytecode` ReadOnlyData
    section bracketed by global symbols
    `__luna_bytecode_start` / `__luna_bytecode_end` (Mach-O
    `_`-prefixed) → system `cc` link with a minimal C entry +
    bytecode `.o` → host-triple native binary that prints the
    embedded section length to `stderr` (proves the section is
    reachable end-to-end).
  - **CLI**: `luna-aot compile <input.lua> [--out <path>]
    [--target <triple>] [--dialect 5.1|5.2|5.3|5.4|5.5|macrolua]`.
    `clap` derive surface; scaffold rejects non-host `--target`
    until Stage 6 cross-compile lands.
  - **Library surface**: `luna_aot::embed::embed_bytecode(source,
    out, target_triple, version)` for programmatic embedders;
    `luna_aot::runtime_stub::aot_main()` (interp-driven Vm
    entry — compiles cleanly, awaiting wire-up to the link step in
    the follow-up session via cargo-bootstrap or staticlib
    distribution per audit § Stage 6 Option A/B); constants
    `BYTECODE_START_SYMBOL` / `BYTECODE_END_SYMBOL` /
    `BYTECODE_SECTION_NAME`.
  - **Supply-chain delta**: `luna-aot` pulls `object 0.36`
    (`default-features = false`, `elf` + `macho` + `pe` + `write_std`)
    + `clap 4` (derive) + dev-only `tempfile 3`. **luna-core
    0-third-party-dep contract is unaffected** — `cargo tree -p
    luna-core --prefix none --no-default-features | grep -cE " v[0-9]"`
    still reports 1. Workspace-wide transitive growth = ~50 crates
    (clap + object + their derive transitives). cargo-deny config
    may want a `[bans] multiple-versions = "warn"` pass; flagged
    for the follow-up phase that adds the per-crate deny job.
  - **Test**: `crates/luna-aot/tests/scaffold_smoke.rs` exercises
    the end-to-end path (parse + compile + dump + `.o` write + `cc`
    link → on-disk non-empty native binary). Does not execute the
    binary — the scaffold's C entry's stderr-only output isn't a
    correctness signal for this session; the runtime-stub follow-up
    adds the stdout-comparison test.
  - **Phase AOT Stage 3 — backend-agnostic lowerer** *(landed in
    this commit)*. Both `lower_int_chunk_into<M: Module>` and
    `lower_trace_into<M: Module>` in `luna-jit::jit_backend` are now
    generic over `cranelift_module::Module`, so the same codegen
    body drives the runtime `JITModule` (live RWX mmap) and the AOT
    `ObjectModule` (`.o` file emission). The JIT-specific module
    construction (`JITBuilder::with_isa` + `builder.symbol("luna_jit_*",
    …)`) is factored into thin helpers `build_jit_module_with_helpers`
    (int-chunk) + `build_trace_jit_module` (trace), keeping
    `JITModule::finalize_definitions` /
    `get_finalized_function` / `TRACE_JIT_HANDLES` insertion isolated
    in the JIT wrappers. The two trace-lowering free fns
    `emit_table_set` / `emit_materialize_live_sunk` are now also
    generic over the module trait. Trace returns place a
    `placeholder_trace_fn` in `CompiledTrace.entry`; the JIT wrapper
    patches the real fn pointer after finalize, while the AOT
    pipeline resolves the symbol at static-link time and never
    invokes `entry` directly. A new smoke test
    `crates/luna-aot/tests/stage3_lower_into_object.rs` drives the
    int-chunk lowerer with `cranelift_object::ObjectModule` and
    asserts the produced bytes carry the host's object-file magic
    number — load-bearing witness that the generic boundary is
    actually consumed by a second backend, not just claimed.
    Helper-symbol registration is JIT-only for now; the AOT pipeline
    will resolve these via static link against a small
    `luna-runtime-helpers` rlib in a follow-up (audit § Stage 3
    Action item 3). 274 / 274 workspace lib tests + 360+ luna-jit
    integration tests stay green; the pre-existing
    `trace_jit_s1` failures (2 / 4, baseline-drift from the TA3
    `trace_enabled = true` ship default) and the known
    `official_run` SIGABRT (IO Safety fd-double-close, see
    `.dev/known-bugs/io-safety-fd-double-close.md`) are unchanged
    by this refactor.
  - **Follow-up sessions** within v1.3 (per
    `.dev/rfcs/v1.3-audit-luna-aot.md` Stages 4-6, ~50 dev-days
    remaining of the 70-day audit estimate): (1) wire
    `runtime_stub::aot_main` into the link step so the binary
    actually `undump`s + runs the embedded bytecode through a `Vm`;
    (2) Cranelift trace mcode emission via `cranelift-object` —
    walk every reachable `Proto`'s hot loops, drive each through the
    new generic lowerer, emit symbols + dispatch table into the AOT
    binary; (3) extract a `luna-runtime-helpers` rlib carrying the
    26 `luna_jit_*` C-ABI helpers so the AOT-linked binary resolves
    them at static-link time (no `JITBuilder::symbol` available
    deploy-side); (4) cross-compile via `--target` + per-triple
    `cc` flags; (5) Alpine no-lua-installed deploy smoke test
    (audit § AOT6).
- **MacroLua dialect support** — Lua syntax extension as an
  optional dialect alongside 5.1-5.5; routed through the existing
  per-dialect lexer/parser machinery so it doesn't disturb the
  PUC compatibility matrix.

### Permanently out-of-scope (decision 2026-06-24)

- **Reclaim `luna` crate name on crates.io** — abandoned; sticking
  with `luna-jit` for the JIT-equipped crate and `luna-core` for
  the 0-dep interpreter. See
  `.dev/discussions/luna-crate-name-history.md`.

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
