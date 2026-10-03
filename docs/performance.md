# Performance

This document records what luna's performance is measured against
and the numbers measured so far. It does **not** publish a
headline `vs LuaJIT 1.21×` or `vs PUC 41/42 green` ratio — both shapes
are perf-methodology anti-patterns, for reasons the rest of this
section spells out:

- "vs subprocess-launched reference" inflates the reference's
  measured time by 50–200 µs of subprocess startup,
  making luna's in-process numbers look better than they are.
- "design ceiling" framing converts unmeasured optimization
  headroom into a permanent excuse.
- "41/42 green" cherry-picks the wins; the one outlier is the
  signal, not the noise.

No public comparison matrix is published. What guards performance
release over release is the CI perf-gate: each push builds the `redis_lua_shape` benchmark for
both the previous release (`PERF_REF` in `perf.yml`) and the pushed commit,
then seven runners each measure every cell as alternating release/commit
pairs over three rounds. A runner's figure for a cell is the median of its
three rounds, and the gate fails when the geometric mean of the seven
figures is more than 3.15% slower than the release. That threshold is set so
that a 5% slowdown in any cell fails with probability of at least 0.95 while
an unchanged commit fails with probability of at most 0.01, so slowdowns of
2-3% fail some of the time. A pushed commit whose message contains
`[perf-allow]` skips the check; for a merge, that is the merge commit's
message. It covers that one workload, not luna's performance in general.

---

## 1. Baselines measured at v1.3

### 1.1 Memory baselines

Five workloads measured under dhat on macOS aarch64:

| Workload | Peak | Steady | Allocs |
|---|---:|---:|---:|
| cold_start (empty Vm) | 33 KB | 31 KB | 435 |
| repl_idle (100 evals) | 71 KB | 69 KB | 2,515 |
| host_roots_churn (1k cycles) | 30 KB | 30 KB | 414 |
| alloc_collect (1M alloc + 10 GC) | 1.0 MB | 523 KB | 555,072 |
| userdata_lifecycle (200 + finalizers) | 73 KB | 63 KB | 1,004 |

Use these as v1.3 regression sentinels — a > 5% steady-state
increase on any workload signals an unintended layout change.

### 1.2 Disk + binary size baselines

Per-crate publish sizes:

| Crate | Files | Raw | Compressed |
|---|---:|---:|---:|
| luna-core | 285 | 4.4 MiB | 1.6 MiB |
| luna-jit | 175 | 1.2 MiB | 286 KiB |
| luna-aot | 47 | 268 KiB | 76 KiB |
| luna-runtime-helpers | 32 | 107 KiB | 31 KiB |
| luna-jit-derive | 6 | 28 KiB | 10 KiB |

AOT output binary sizes:

| Script | Dev | Release | Release-stripped |
|---|---:|---:|---:|
| `hello.lua` (1 line) | 12.4 MiB | 6.0 MiB | 4.5 MiB |
| `fib.lua` (fib_28) | 12.4 MiB | 6.0 MiB | 4.5 MiB |
| `production_like.lua` (~1.5k LOC) | 12.5 MiB | 6.1 MiB | 4.6 MiB |

### 1.3 Compile-time perf

luna-core (interp-only) builds in seconds on a stock laptop. The
0-third-party-dep contract is the dominant cost driver here:
embedders pulling only `luna-core` skip ~30 transitive Cranelift
crates, and the `cargo deny check` CI gate enforces the contract
on every PR.

### 1.4 Runtime hot-path counters

Not headline numbers, but useful for diagnosing whether a workload
is getting JIT speedup:

```rust
let count = vm.trace_compiled_count();
let dispatches = vm.trace_dispatched_count();
let aborts = vm.trace_aborted_count();
let deopts = vm.trace_deopt_count();
```

A workload where `trace_dispatched_count` stays low while
`trace_aborted_count` climbs is hitting a recorder limit
(e.g. inline depth or trace length). The limits are in
`crates/luna-jit/src/jit_backend/`.

## 2. Tuning knobs

For workload-shape-specific tuning, see
[`deploy.md`](deploy.md) §3. Briefly:

| Knob | Default | Effect |
|---|---|---|
| `vm.set_jit_enabled(false)` | `true` (luna-jit) | Disable for predictable latency / debug repro |
| `vm.set_trace_jit_enabled(false)` | `true` (v1.3 TA3 default) | Disable to A/B trace JIT vs interpreter |
| `vm.set_hot_threshold(n)` | (recorder constant) | Lower for hot-immediately workloads; raise for cold-data services |
| `vm.set_max_trace_len(n)` | (recorder constant) | Raise for long unrolled loops; lower for diverse-shape recording |
| `vm.set_trace_tier(tier)` | `TraceTier::Auto` (`LUNA_TRACE_TIER`) | `Auto` compiles a trace with the baseline code generator first and with Cranelift once it is hot; `Baseline` / `Optimizing` keep one of them |
| `vm.set_trace_tier_up_at(n)` | 16384 | Loop iterations in baseline code before Cranelift recompiles the trace, a quarter of that once the trace's function is called again (`0`: never) |

## 3. The benchmark harnesses

The `cross_dialect` and `redis_lua_shape` bench harnesses in
`crates/luna-jit/benches/` run with `cargo bench --bench cross_dialect`
and `cargo bench --bench redis_lua_shape`. Read their numbers with two
caveats:

- PUC reference times include subprocess startup; treat them as
  upper bounds, not as the actual VM cost.
- The cells were chosen to surface luna's wins, not to span the
  workload-shape space.

## 4. See also

- [`architecture.md`](architecture.md) — steel/cement/stone
  classification + crate layout
- [`deploy.md`](deploy.md) — runtime tuning knobs
- [`binary-size.md`](binary-size.md) — per-crate + AOT-output budget
  + reduction levers

---

*The v1.0 perf table is in git history at commit
[`262c705`'s `docs/performance.md`](https://github.com/goliajp/luna/blob/262c705/docs/performance.md).*
