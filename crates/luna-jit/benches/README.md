# luna-jit benches

Criterion-based perf benchmarks for luna-jit. All benches use
`harness = false` and run via `cargo bench --bench <name>`.

luna-core deliberately ships **zero third-party deps** (CI `zero-dep`
job enforces); criterion lives in luna-jit's `dev-dependencies` only.
Benches whose subject is a luna-core property still live in this crate
for that reason — they pull the bench harness from here and exercise
luna-core through the re-exported `luna_jit::*` surface.

## Benches

### `lua_microbench`

Academic-shape microbenches (`fib_28`, `loop_int_1m`, etc). The classic
"language perf comparison" workloads. Lives next to `cross_dialect` for
cross-dialect (5.1/5.2/5.3/5.4/5.5) coverage.

### `cross_dialect`

Same workloads as `lua_microbench`, parameterised across the 5.x
dialects. Catches cross-dialect perf regressions (e.g. a 5.4
integer-arith change that pessimises the 5.1 path).

### `redis_lua_shape`

The **real-embedder** workload shape (token bucket, sliding window,
method dispatch via metatables, string ops). The dogfood report
established this as the load-bearing perf surface for luna's embedder
use case. Variance gate: 2.5 % on macOS local (no public CPU pin
API on Apple Silicon); ~1-2 % on Linux CI via `taskset -c 1`
(`.github/workflows/ci.yml` `perf-gate` job).

### `run_only`

Walltime smoke run for the official Lua test corpus snippets.

### `bench_send_overhead` (v1.3 SS-A)

**Measures the irreducible cost of wrapping `Vm` behind a Send-shaped
indirection layer**, before any real Arc-of-fields / RwLock semantics
land. The wrapper is `Arc<UnsafeCell<Vm>>` plus `unsafe impl Send` — a
shape-only no-op (same `Vm`, same `NonNull<T>` GC handles, same
single-mutator dispatcher; only difference is one `Arc` ptr-load and
one `UnsafeCell::get()` per outer `eval` call).

Bench cases (four, in two pairs):

| Case | Workload | Vm path |
|---|---|---|
| `bare_vm_eval` | `return 1+2` × 10k iters | direct `Vm::eval` |
| `wrapped_vm_eval` | same | via `NoOpSendWrapper` |
| `bare_vm_token_bucket` | Redis-Lua token bucket (1k iters) | direct `Vm::eval` |
| `wrapped_vm_token_bucket` | same | via `NoOpSendWrapper` |

**Interpreting the output**: take the ratio
`wrapped / bare` in each pair. That's the **framework tax** the
SS-B `SendVm` newtype-fork implementation will pay on top of (or
instead of) the per-`Gc<T>` deref cost decomposed in
`.dev/rfcs/v1.2-audit-send-cost.md`.

Audit projections from `.dev/rfcs/v1.3-audit-send-vm-design.md` §3.2:

| Arch | Per-deref `Arc<UnsafeCell<T>>` cost | Full SendVm B2 projection |
|---|---|---|
| ARM M-series | ~3 ns | ~3 % |
| x86_64 Linux | ~6 ns | ~6 % |

This bench measures **only** the outer Vm-handle indirection, **not**
the per-`Gc<T>` deref cost (a future luna-core bench picks up that
side). If `wrapped_vm_token_bucket` overhead exceeds **5 %** on macOS,
**that is a loud flag** — it means the SS-B projection is optimistic
and SS-B should re-scope before any source mutation.

**Linux x86_64**: macOS is the dev measurement. For the canonical
Linux x86_64 number (the harder arch per `v1.2-audit-send-cost.md`
§3.2 — wider per-deref cost gap there), dispatch the CI `perf-gate`
workflow against this bench name. The variance band there is ~1-2 %
via `taskset -c 1`.

## Running

```sh
cargo bench --bench bench_send_overhead              # full
cargo bench --bench bench_send_overhead -- --quick   # fast sanity (skip stats)
cargo bench --bench bench_send_overhead -- wrapped_vm_token_bucket  # filter
```

Baseline captures live under `.dev/perf-baselines/<date>-<topic>.md`.
