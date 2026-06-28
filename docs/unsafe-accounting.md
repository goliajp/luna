# Unsafe accounting

luna uses `unsafe` Rust in a small handful of well-bounded categories:
the GC heap's `NonNull`-based pointer model, the JIT backend's FFI to
Cranelift-emitted machine code, the `lua.h`-compatible C ABI, the
AOT-binary runtime entry, and the cross-thread `feature = "send"`
SendVm newtype. Every `unsafe` block carries a `SAFETY:`
justification.

This page is the human-readable companion to `cargo-geiger`'s
machine-grepable summary. Run `cargo geiger -p luna-jit --bin luna`
for the tool's per-crate breakdown; the numbers and pattern
explanations below give the equivalent picture without the install.

For the **embedder-surface contract** (0 `unsafe` required to use
`luna-core` or `luna-jit`'s public API) see
[`security.md`](security.md) §5.

---

## 1. Current snapshot (v1.3.0 ship + v2.0 dev sprint)

| Metric | Count | Notes |
|---|---:|---|
| First-party `unsafe` sites in CI-watched scope (`luna-core` + `luna-jit`) | **490** | CI ceiling at `.github/workflows/ci.yml::unsafe-drift` |
| Sites in `luna-runtime-helpers` (AOT-binary runtime entry) | **18** | C ABI + linker bracket-symbol walkers; out of CI ceiling scope |
| Sites in `luna-aot` (AOT compiler) | **1** | Lone `unsafe extern` block for trace mcode handoff |
| Sites in `luna-jit-derive` (proc macro) | **0** | Pure expansion logic |
| Sites in `luna-tools` (dev tools) | **0** | Pure-read accessors via safe `vm::inspect` |
| **`pub unsafe fn` in public API** | **4** | All `#[doc(hidden)]` — never surface in `cargo doc` |
| **`unsafe impl Send/Sync`** | **~7** | 5 baseline (v1.1) + 2 v1.3 (SendVm newtype + TB trace_fn) |

**v1.1 ship snapshot** was 461 first-party sites; v1.3+v2.0 adds
~30 sites for SendVm + Trace-bearing userdata (TB) + AOT trace
metadata wiring + async hook composition (AS) + Wave 1 PUC shared
helpers' inline `unsafe` for register-bound arithmetic. CI ceiling
is **490 with 0 slots of headroom** — any new unsafe must either
land with an explicit ceiling bump in `ci.yml` and a justified
SAFETY comment, or refactor to safe equivalent. Never widen to a
round number to silence drift.

## 2. Distribution by crate (v1.3+v2.0)

```
crates/luna-core/src/                    267  (was ~285 at v1.1 — net drop from
  runtime/heap.rs            ~110         compiler short-circuit fix + Wave 1
  runtime/value.rs            ~25         shared helper extraction)
  runtime/userdata.rs         ~25         (+10 for TB trace_fn downcast)
  vm/exec.rs                 ~100         (incl. v1.3 LUAI_COMPAT_VARARG cold-path)
  vm/dump/puc/*.rs            ~10         (PU Wave 2/3/4 register-bound arithmetic)
  vm/builtins.rs              ~15
  vm/lib_os_io.rs             ~10
  vm/lib_strpack.rs            ~5
  vm/typed_native.rs           ~5
  vm/inspect.rs                ~2         (v2.0 TL pure-read accessors)

crates/luna-jit/src/                     152  (was ~185 at v1.1 — net drop from
  jit_backend/mod.rs         ~75         JIT_CACHE TLS scope tightening)
  jit_backend/trace.rs       ~70         (PU Wave 1 helpers + Stage 7 polish 6
                                          chain reloc paths)
  send_vm.rs                  ~3         (v1.3 SendVm newtype Send/Sync impls)
  capi.rs                    ~30

crates/luna-runtime-helpers/src/          18  (NEW in v1.3 for AOT-binary runtime)
  lib.rs                                   AOT entry symbol + bracket-walker
                                           reads of __start_/__stop_ /
                                           Mach-O section$start$ symbols

crates/luna-aot/src/                       1  (NEW in v1.3, single trace lower
                                              extern wrapper)
```

## 3. Pattern catalog (why these unsafe blocks exist)

### 3.1 `Gc<T>` deref (~250 of 490)

luna's GC handles are `Gc<T> = NonNull<T>` over an intrusive
mark-sweep heap. Every read or write through a handle is a
`unsafe { *gc.as_ptr() }` or `unsafe { &mut *gc.as_ptr() }`.

The safety contract is documented in `runtime/heap.rs:5-7`:
*"the runtime is single-threaded; a `Gc` pointer is valid until
a `collect()` call that does not reach it from the given roots."*
This invariant is enforced by:

- `Vm: !Send + !Sync` (default; CB doctest at A7 guards regression)
- The Vm's `gc_roots()` aggregator covering every reachable handle
  (host_roots, globals, stack, frames, metatables, hook function,
  current coroutine, etc.)
- `Vm: !Send` prevents cross-thread access; the single-threaded
  contract holds by construction. For cross-thread use, see
  `SendVm` (v1.3 newtype with `Arc<UnsafeCell<Vm>>` + `RwLock`
  fast/slow path).

### 3.2 JIT FFI thread-locals (~80 of 490)

When Cranelift-compiled code calls back into Rust via
`luna_jit_*` extern "C" helpers, those helpers need to reach the
active Vm. luna stores `&mut Vm` in a thread-local (`JIT_VM`) at
dispatch entry; the helpers read it back as
`&mut *JIT_VM.with(|c| c.get())`. The `JitVmGuard` RAII type
holds the Vm pointer for the duration of a JIT slice; SAFETY
annotations cite the guard's lifetime as the validity proof.

The v2.0 Track J audit (`.dev/rfcs/v2.0-plan-state.md` §Track J)
plans to move this off `thread_local!` onto a `Vm.VmJitStorage`
field for cross-thread JIT — gated on Cranelift's
`JITModule: Send` confirmation.

### 3.3 `unsafe extern "C" fn` helpers (~30 of 490)

27 `luna_jit_*` helpers in `crates/luna-jit/src/jit_backend/mod.rs`
have stable C ABI shapes Cranelift can call. Each is annotated
`#[unsafe(no_mangle)]` (Rust 2024 edition form) so the linker
exposes the symbol; SAFETY rationale at each helper cites the
codegen contract Cranelift establishes (specific register state,
stack layout).

The AOT pipeline (v1.3 Stage 7 polish 1) verified all 27 helpers
are correctly linker-dead-stripped in produced binaries.

### 3.4 `Box::into_raw` / `Box::from_raw` pairs (~20 of 490)

Used for transferring ownership of heap-allocated trace metadata
between Cranelift's symbol table and luna's trace cache. Each
`into_raw` is matched by exactly one `from_raw` on the same
allocator path; cleanup runs when the trace is evicted.

### 3.5 v1.3 additions

- **SendVm newtype** (`crates/luna-jit/src/send_vm.rs`, ~3 sites)
  — `unsafe impl Send` / `unsafe impl Sync` on
  `SendVm(Arc<UnsafeCell<Vm>>)` gated `feature = "send"`. SAFETY
  rationale: the `RwLock` guard enforces single-mutator access
  across threads; interior `Vm` keeps its single-threaded
  invariant under each lock acquisition.
- **TB trace_fn** (`crates/luna-core/src/runtime/userdata.rs`,
  ~5 sites added) — `unsafe { fn_ptr(payload, marker) }` for
  monomorphic Trace-bearing host payloads. SAFETY: trace_fn is
  installed only via the typed `LuaUserdata::trace` impl
  expansion; payload Any-downcast is type-stable per `TypeId`.
- **AOT runtime helpers** (`crates/luna-runtime-helpers/src/`,
  18 sites total — NEW crate) — bracket-symbol walkers for
  `__start_/__stop_` (ELF), `section$start$` (Mach-O), and PE
  section table walk (Windows). Each reads kernel-supplied
  linker symbols; SAFETY: the linker contract guarantees the
  bracket pair is contiguous and non-overlapping.
- **AS async natives** (`crates/luna-core/src/vm/exec.rs`,
  ~3 sites added) — `Call` before stash future + `Return` after
  commit hook firing for async-native dispatch. SAFETY: future
  is stashed before any cross-await reads of `&mut Vm`.

### 3.6 Public `pub unsafe fn` (4 total — unchanged from v1.1)

| File:line | Function | Why |
|---|---|---|
| `runtime/heap.rs:130` | `Gc::<T>::as_mut` | `#[doc(hidden)]`. Internal mutation; embedders use `TableBuilder` / `LuaUserdata` builder. |
| `runtime/value.rs:133` | `Value::as_closure_unchecked` | `#[doc(hidden)]`. JIT hot-path; bypasses tag match. Safe alternative: `Value::Closure(_)` match arm. |
| `runtime/value.rs:156` | `Value::as_int_unchecked` | Same shape. |
| `runtime/value.rs:296` | `Value::pack` | `#[doc(hidden)]`. Low-level Value constructor used by JIT codegen + capi. |

None of these appear in `cargo doc`'s public API surface; the
`#[doc(hidden)]` ensures embedders following the rustdoc never
discover them.

### 3.7 `unsafe impl Send/Sync` (7 total — v1.3 +2 from v1.1's 5)

| File:line | Impl | Why |
|---|---|---|
| `runtime/function.rs:419-420` | `LuaClosure: Send + Sync` | `LuaClosure` itself is `Send`-compatible (no `Rc`/`RefCell`); `Gc<LuaClosure>` is `!Send` separately. |
| `runtime/table.rs:120-121` | `Table: Send + Sync` | Same shape — Table itself is `Send`-compatible, `Gc<Table>` is `!Send`. |
| `jit_backend/trace.rs:1892` | `TraceHandle: Send` | Required by `thread_local!`'s `RefCell<Vec<TraceHandle>>` bound. TLS context guarantees single-thread access. |
| `jit_backend/trace.rs` (chain-inline) | `InlineChainSlot: Send + Sync` | v1.3 AOT Stage 7 polish 6 chain reloc slot type. SAFETY: AOT binary slots are populated once at link time. |
| `send_vm.rs` | `SendVm: Send + Sync` | v1.3 SendVm newtype. SAFETY: RwLock enforces single-mutator; interior `Vm` single-threaded under each lock. |

Tracked in `.dev/rfcs/v1.3-audit-send-vm-design.md` §1 for
SendVm rationale + `.dev/rfcs/v1.3-audit-trace-bearing-userdata.md`
for the Send/Sync shape of monomorphic trace_fn.

## 4. CI enforcement

The `unsafe-drift` job in `.github/workflows/ci.yml::230` counts
first-party `unsafe` sites in `crates/luna-core/src` +
`crates/luna-jit/src` on every PR. Ceiling is **490** (0 headroom);
each justified new unsafe lands with:

1. SAFETY: comment on the block (or `unsafe fn` declaration if the
   block lives inside an `unsafe fn`)
2. Either ceiling bump in `ci.yml` (with commit-message rationale)
   OR offsetting refactor that removes equivalent count from
   another file

The drift-detection scope intentionally excludes `luna-runtime-helpers`
(C ABI + linker walkers — different invariant shape) and
`luna-aot` (build-time tool — not in runtime audit surface).

## 5. Reproducing with cargo-geiger

```sh
cargo install --locked cargo-geiger
cargo geiger -p luna-jit --bin luna
```

The tool's output groups unsafe by category (`unsafe expressions`,
`unsafe traits`, `unsafe functions`, `unsafe impls`) and shows
per-dependency unsafe counts so embedders can audit the supply
chain. **luna-core has zero third-party deps** so its unsafe
footprint is entirely first-party.

Quick first-party count without the tool:

```sh
grep -rE 'unsafe (\{|fn |impl |trait |extern )' \
    crates/luna-core/src crates/luna-jit/src | wc -l
# Expected: 490 (or whatever ceiling is in ci.yml's unsafe-drift job)
```

## 6. Ship-time gates

- **A6 ship gate**: SAFETY: comment coverage to 100% on
  `unsafe { ... }` blocks. Shipped at v1.1 commit `7d4a95e`
  (342 comments added; 0 TODO(audit) placeholders).
- **A4 ship gate**: 0 `unsafe` at the embedder surface (public
  `cargo doc` view). Shipped at v1.1 commit `37e9414` and held
  through v1.3 ship — the 4 `pub unsafe fn` are `#[doc(hidden)]`.
- **v1.3 ship gate**: unsafe-drift count = 490 with 0 headroom.
  Held; any v2.0 increase requires explicit ceiling bump in
  `ci.yml`.

## 7. See also

- [`security.md`](security.md) — embedder-surface contract +
  threat model
- [`architecture.md`](architecture.md) §3 — steel/cement/stone
  classification (the `unsafe`-dense stone tier is where the
  `Gc<T>` invariants live)
- `.dev/rfcs/v1.3-audit-send-vm-design.md` — SendVm Send/Sync
  rationale
- `.dev/rfcs/v1.3-audit-trace-bearing-userdata.md` — TB trace_fn
  monomorphic dispatch SAFETY
- `~/.claude-shared/global/principles.md` §`code/no-unsolicited-*`
  — the project-wide stance against speculative `unsafe`

---

*Last refreshed 2026-06-25 for the v1.3.0 ship + v2.0 dev sprint
state. Numbers measured via the §5 grep recipe on develop tip
`6f939bc`. The CI ceiling (`ci.yml::unsafe-drift`) is the
load-bearing version of this snapshot; this page exists to
explain `why` rather than `what`.*
