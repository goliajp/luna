# Production deployment

This document covers the deployment-time concerns of running luna
in production: which crate to pull, packaging shapes, configuration
knobs, observability hooks, and graceful-shutdown patterns.

For build-time AOT compile (Lua source → standalone native binary)
see [`aot.md`](aot.md). For the threat model that frames each opt-in
see [`security.md`](security.md). For embedding cookbooks see
[`embedding.md`](embedding.md).

---

## 1. Which crate to pull

| Deployment shape | Crate | Notes |
|---|---|---|
| Static-linked native binary, frozen Lua source | `luna-aot` (build-time) → standalone binary | No runtime crate needed on host |
| Rust service embedding `Vm`, JIT enabled | `luna-jit` | Pulls Cranelift; cross-thread via `feature = "send"` |
| Rust service embedding `Vm`, interpreter only | `luna-core` | Zero third-party deps; ~280 KiB compressed |
| WASM (browser / wasm runtimes) | `luna-core` + `--target wasm32-wasip1` | JIT off; `io`/`os` stubbed |
| Cross-thread fleet (tokio JoinSet, web workers) | `luna-jit` + `feature = "send"` | See [`threading.md`](threading.md) |

The four library crates (`luna-core`, `luna-jit-derive`, `luna-jit`,
`luna-runtime-helpers`) compose: `luna-jit` re-exports `luna-core`
+ adds Cranelift + REPL, so you don't dual-pull. `luna-aot` is
build-time only; the binary it produces statically links
`luna-runtime-helpers`.

---

## 2. Packaging shapes

### 2.1 Single standalone binary (AOT)

```sh
luna-aot compile service.lua --target x86_64-unknown-linux-gnu --out service
# ./service runs without luna on the host
```

Suitable for: edge functions, embedded devices, container images
where you don't want a Rust toolchain or `luna` install in the
image. The produced binary is fully self-contained (statically
linked); see [`aot.md`](aot.md) for binary size + section
breakdown.

### 2.2 Static-linked Rust service

```toml
# Cargo.toml of the host crate
[dependencies]
luna-jit = { version = "1.3", default-features = false, features = [] }
```

The default-features-off shape skips the REPL line editor (only
needed for interactive `luna` REPL). Service code wraps `Vm`:

```rust
let mut vm = luna_jit::Vm::new(luna_jit::LuaVersion::Lua55);
vm.open_os_io();  // or omit for sandboxed
let r: i64 = vm.eval("return 1 + 2").unwrap();
```

See [`embedding.md`](embedding.md) for the full builder cookbook.

### 2.3 Container image

```dockerfile
FROM scratch
COPY ./target/x86_64-unknown-linux-musl/release/service /service
ENTRYPOINT ["/service"]
```

For the smallest possible image, build with
`--target x86_64-unknown-linux-musl` (statically linked against
musl, no glibc dependency). Add `RUN strip /service` before the
COPY in a multi-stage build to drop ~85% of `__LINKEDIT` from
the binary.

### 2.4 WASM (browser / wasmtime / wasmer)

```toml
[dependencies]
luna-core = { version = "1.3" }   # interp only — no Cranelift on wasm
```

`io.popen` / `os.execute` are compiled out under `wasm32-wasip1`
(see commit `e72a43e` — `--target` cfg gating). Sandbox boundaries
come from the wasi runtime, not from luna.

---

## 3. Configuration knobs

Runtime tunables live on `Vm`:

| Knob | Default | When to change |
|---|---|---|
| `vm.set_jit_enabled(bool)` | `true` (luna-jit) | Disable for predictable startup latency / debug repro |
| `vm.set_trace_jit_enabled(bool)` | `true` (v1.3 TA3) | Disable to A/B trace vs no-trace perf |
| `vm.set_hot_threshold(u32)` | (interpreter heuristic) | Lower for hot-immediately workloads; raise for cold-data services |
| `vm.set_max_trace_len(u32)` | (per-recorder constant) | Raise for long unrolled loops; lower for diverse-shape recording |
| `vm.allow_bytecode_loading()` | off | Enable only for fully trusted bytecode (security boundary) |
| `vm.open_os_io()` | not called | Trusted hosts only — see [`security.md`](security.md) |
| `vm.open_debug()` | not called | Debugging hooks, hot-path overhead |
| `vm.open_package()` | not called | Trusted source path config |

All knobs are sticky — set them once at startup, then `eval`.

---

## 4. Observability

### 4.1 Logging

luna doesn't log internally. Embedders hook via the **debug hook**
mechanism (PUC-compatible `debug.sethook`):

```rust
vm.set_rust_debug_hook(|vm, event| {
    log::trace!("luna event: {:?} at pc={}", event, vm.current_pc());
});
```

Per-thread; install on the main `Vm` and per-coroutine `Vm` clones
separately. See `crates/luna-jit/examples/async_host.rs` for the
async pattern.

### 4.2 Metrics

Expose via host counters around `Vm::eval`:

```rust
let start = std::time::Instant::now();
let result = vm.eval(script);
metrics::histogram!("luna.eval.duration_ms", start.elapsed().as_millis() as f64);
metrics::counter!("luna.eval.error", if result.is_err() { 1 } else { 0 });
```

For JIT-specific counters (`trace_compiled_count`, `trace_aborted_count`,
`trace_dispatched_count`), read via `vm.trace_*_count()` accessors;
useful for diagnosing why a workload isn't getting JIT speedup.

### 4.3 Memory baseline

The dhat-based mem baseline lives at
`.dev/baselines/mem-2026-06-25/` (gitignored). Reproduce with the
recipe in [`contributing-mem.md`](contributing-mem.md); expected
peaks at v1.3 ship for the 5 measured workloads:

| Workload | Peak | Steady |
|---|---:|---:|
| cold_start (empty Vm) | 33 KB | 31 KB |
| repl_idle (100 evals) | 71 KB | 69 KB |
| host_roots_churn (1k cycles) | 30 KB | 30 KB |
| alloc_collect (1M evals + 10 GC) | 1.0 MB | 523 KB |
| userdata_lifecycle (200 + finalizers) | 73 KB | 63 KB |

Use these as regression sentinels — a >5% steady-state increase
on any workload should raise an alarm.

---

## 5. Graceful shutdown

### 5.1 Sync embedder

```rust
let result = vm.eval(script);
drop(vm);  // runs all pending __gc finalizers via Heap::drop
```

`__gc` finalizers always run on `Vm::drop`. If a finalizer panics,
the next finalizer still runs (PUC semantics; see
`cb_edge_gc_finalizer.rs::gc_finalizer_error_does_not_abort_program`).

### 5.2 Async embedder

```rust
let fut = vm.eval_async(script);
// On shutdown:
drop(fut);  // cancels the future; closeable iterators get __close
drop(vm);   // runs remaining __gc finalizers
```

`EvalFuture::Drop` runs to-be-closed (`__close`) handlers on
locals that have them — important for DB transactions or file
handles held in Lua locals.

For tokio-driven fleets:

```rust
tokio::time::timeout(Duration::from_secs(5), vm.eval_async(script)).await
```

On timeout, the future is dropped; the in-progress coroutine
gets `__close` cleanup. **Note**: per the v2.0 charter Track AT
(async tokio first-class), the cancellation invariant for TBC
inside `EvalFuture::Drop` is being audited — pin the behavior
in your own integration tests against the luna version you ship.

---

## 6. Cross-thread deployment

`feature = "send"` on `luna-jit` enables the `SendVm` newtype —
`Arc<UnsafeCell<Vm>> + RwLock` outer shape, JIT path interp-only at
v1.3. See [`threading.md`](threading.md) for the tokio JoinSet
pattern + per-request `Vm` pool sizing recipe.

JIT-aware cross-thread (Track J in v2.0) is **not yet ship**;
interp-only SendVm has zero overhead measured (-1.8% to -0.3% on
M4 Max under SS-A bench) but trace mcode does not currently move
across threads.

---

## 7. Upgrade considerations

When upgrading between minor versions (`1.x` → `1.y`):

- All `pub` items in `lib.rs`'s exported tree are stable per the
  semver-major contract.
- `lua.h`-compatible C ABI in `src/capi.rs` is stable.
- Bytecode binary format (per dialect) is stable; PUC `.luac` files
  load in/out across `1.x`.
- Internal modules (JIT codegen, dispatcher hot-path, heap internals)
  may change without notice.

When upgrading across major versions (`1.x` → `2.0`):

- See [`migration-v1-to-v2.md`](migration-v1-to-v2.md) for the
  per-area breaking-change checklist + migration recipes.

---

## 8. See also

- [`aot.md`](aot.md) — single-binary deploy mode
- [`embedding.md`](embedding.md) — library embed cookbook
- [`threading.md`](threading.md) — cross-thread + tokio
- [`security.md`](security.md) — threat model + sandbox boundaries
- [`binary-size.md`](binary-size.md) — per-crate + AOT-output budgets
- [`compatibility.md`](compatibility.md) — per-dialect feature matrix
- [`contributing-mem.md`](contributing-mem.md) — memory baseline
  reproduction
- [`migration-v1-to-v2.md`](migration-v1-to-v2.md) — major-version
  upgrade checklist
