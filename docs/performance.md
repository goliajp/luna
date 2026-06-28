# Performance

This document tracks luna's performance discipline, methodology, and
what's measured at the current ship. It does **not** publish a
headline `vs LuaJIT 1.21×` or `vs PUC 41/42 green` ratio — both shapes
are perf-methodology anti-patterns (see §1 "trigger words" in
`~/.claude-shared/global/methodology/perf-decomposition-vs-polish.md`,
the project-wide perf attack methodology):

- "vs subprocess-launched reference" inflates the reference's
  measured time by 50–200 µs of subprocess startup,
  making luna's in-process numbers look better than they are.
- "design ceiling" framing converts unmeasured optimization
  headroom into a permanent excuse.
- "41/42 green" cherry-picks the wins; the one outlier is the
  signal, not the noise.

Public bench numbers ship with the v2.0 release per Track BM
(`.dev/rfcs/v2.0-plan-state.md` §Track BM summary). The matrix
will compare luna_jit / luna_interp / luna_aot vs LuaJIT 2.1 +
PUC 5.4 + mlua across 13 workloads × 3 host targets, with
in-process measurement boundaries and ±err bars on every cell.
**BM is sequenced LAST** in the v2.0 phase order specifically to
prevent baseline lock-in around currently-suboptimal attack
surfaces.

---

## 1. Methodology

luna's perf attack methodology is documented in
`~/.claude-shared/global/methodology/perf-decomposition-vs-polish.md`
and applied across the v2.0 sprint. Key principles:

- **Decomposition before polish.** Any gap > 1.5× a reference impl
  triggers a side-by-side 18-stage decomposition of the workload,
  not surface-level polish iterations. The v2.0 Track PI audit
  (`.dev/rfcs/v2.0-audit-perf-interp-gap.md`, full 26 KB body
  preserved) walks the methodology explicitly for the interp gap
  close work.
- **Measure both axes.** `luna_jit vs LuaJIT_jit` and `luna_interp
  vs LuaJIT_interp` are independent dimensions. Conflating them
  hides the attack surface.
- **Don't punish the workload.** If a gap exists, it's an
  optimization opportunity, not a "workload not amenable" verdict.
  See methodology §1 trigger-word list for the full set of
  rationalizations that get flagged in code review.

## 2. What's measured today (v1.3 ship)

### 2.1 Memory baselines

`.dev/baselines/mem-2026-06-25/` (reproducible via
[`contributing-mem.md`](contributing-mem.md)). Five workloads measured
under dhat on macOS aarch64:

| Workload | Peak | Steady | Allocs |
|---|---:|---:|---:|
| cold_start (empty Vm) | 33 KB | 31 KB | 435 |
| repl_idle (100 evals) | 71 KB | 69 KB | 2,515 |
| host_roots_churn (1k cycles) | 30 KB | 30 KB | 414 |
| alloc_collect (1M alloc + 10 GC) | 1.0 MB | 523 KB | 555,072 |
| userdata_lifecycle (200 + finalizers) | 73 KB | 63 KB | 1,004 |

Use these as v1.3 regression sentinels — a > 5% steady-state
increase on any workload signals an unintended layout change.

### 2.2 Disk + binary size baselines

`.dev/baselines/disk-2026-06-25/` (reproducible via
[`contributing-disk.md`](contributing-disk.md)). Per-crate publish
sizes:

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

Mach-O section breakdown for `production_like.lua` release-stripped
at `.dev/baselines/disk-2026-06-25/macho-sections.md`.

### 2.3 Compile-time perf

luna-core (interp-only) builds in seconds on a stock laptop. The
0-third-party-dep contract is the dominant cost driver here:
embedders pulling only `luna-core` skip ~30 transitive Cranelift
crates, and the `cargo deny check` CI gate enforces the contract
on every PR.

### 2.4 Runtime hot-path counters

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
(e.g. inline depth or trace length). See `crates/luna-jit/src/jit_backend/`
for the limits and `.dev/rfcs/v2.0-audit-perf-interp-gap.md` for
the methodology used to find them.

## 3. Tuning knobs

For workload-shape-specific tuning, see
[`deploy.md`](deploy.md) §3. Briefly:

| Knob | Default | Effect |
|---|---|---|
| `vm.set_jit_enabled(false)` | `true` (luna-jit) | Disable for predictable latency / debug repro |
| `vm.set_trace_jit_enabled(false)` | `true` (v1.3 TA3 default) | Disable to A/B trace JIT vs interpreter |
| `vm.set_hot_threshold(n)` | (recorder constant) | Lower for hot-immediately workloads; raise for cold-data services |
| `vm.set_max_trace_len(n)` | (recorder constant) | Raise for long unrolled loops; lower for diverse-shape recording |

## 4. v1.x → v2.0 perf evolution

v1.x perf headlines were historically published as "`vs.X = luna_time
/ X_time` ≤ 0.50 on 41/42 cells" — this framing is the cherry-pick
optics + subprocess-startup-inflation pair flagged above. v2.0
replaces it with the BM matrix described in
`.dev/rfcs/v2.0-plan-state.md` §Track BM (13 workloads × 6 VMs × 3
host targets, in-process measurement, ±err bars).

The historical `cross_dialect` + `redis_lua_shape` bench harnesses
in `crates/luna-jit/benches/` still run (`cargo bench --bench
cross_dialect` / `cargo bench --bench redis_lua_shape`), but their
numbers should be interpreted in light of:

- PUC reference times include subprocess startup; treat them as
  upper bounds, not as the actual VM cost.
- The cells were originally chosen to surface luna's wins, not to
  span the workload-shape space. The v2.0 BM matrix corrects this.

For the v2.0 sprint's perf attack on the interp gap specifically, see
`.dev/rfcs/v2.0-audit-perf-interp-gap.md` for the file:line attack
targets (`vm/exec.rs:7739-7773` newindex double-walk / `:6706-6709`
Move opcode / `:6705` dispatcher) and the 18-stage decomposition
scaffold for `token_bucket_1k`.

## 5. See also

- `~/.claude-shared/global/methodology/perf-decomposition-vs-polish.md`
  — full perf attack methodology (§1 trigger words / §3 sprint
  structure / §7 luna v2-J Chain A实证 lessons)
- [`contributing-mem.md`](contributing-mem.md) — memory baseline
  reproduction
- [`contributing-disk.md`](contributing-disk.md) — disk + binary
  size baseline reproduction
- [`architecture.md`](architecture.md) — steel/cement/stone
  classification + crate layout
- [`deploy.md`](deploy.md) — runtime tuning knobs
- [`binary-size.md`](binary-size.md) — per-crate + AOT-output budget
  + reduction levers

---

*v1.0 perf table archived in git history at commit
[`262c705`'s `docs/performance.md`](https://github.com/goliajp/luna/blob/262c705/docs/performance.md).
Live numbers ship with v2.0 release per Track BM.*
