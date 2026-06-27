# Phase 1J.2 — PMU probe attempt notes

**Host**: Apple Silicon (`uname -mr` = `25.5.0 arm64`), macOS Darwin 25.5.0.
**Worktree base**: `53c042d` (develop tip post 1I.D revert).
**Scope**: probe attribution of the unattributed ~16 µs/cell residual gap from Phase 1I.D § 3.1 (1-site IC env-ON regresses ~21 µs/cell vs env-OFF; asm decomp accounts for ~5 µs; remaining ~16 µs unexplained at asm level).

## Tool inventory

| Tool | Status | Note |
|---|---|---|
| `dtrace` | SIP-restricted | `csrutil status` reports `enabled`; PMC providers require partial-SIP-disable to load. Out of scope for a read-only audit. |
| `instruments` (Instruments.app CLI) | NOT INSTALLED | `which instruments` empty. Full Xcode would install it; we have command-line-tools only (`xcrun --version` = `72`). |
| `xctrace` | AVAILABLE | `/usr/bin/xctrace`; shipped with command-line-tools. |
| `xctrace list templates` includes | — | `CPU Counters`, `CPU Profiler`, `Processor Trace`, `System Trace`. |

## Probe feasibility

`xctrace record --template "CPU Counters" --launch -- <luna binary> ...` would record Apple PMC samples (L1D miss, L2 miss, branch mispredict, LSU stall) across a token_bucket_1k run window. Output is a `.trace` bundle directory.

Without Instruments.app, the bundle has to be exported via `xctrace export --input <trace> --xpath '/trace-toc/*'` and parsed manually. The XML schema is sparsely documented; per-counter PMC-event-id mapping requires the Apple Silicon PMU event reference (M1/M2/M3 series differ).

## Decision

**Probe NOT executed in Phase 1J.** Rationale:

1. xctrace path is viable but consumes the full Phase 1J time budget (build luna release + record N≥30 token_bucket_1k iterations under xctrace + export + manual XML parse + cross-correlate per-PMU-event µs attribution). That is bench-class work, not a 45-60 min audit step.
2. The 3 candidate audits (Phase 1J.3 / 1J.4 / 1J.5) can be performed hypothesis-only against the existing Phase 1I.D § 3.1 attribution (I-cache locality / LSU pipeline / key-const materialization). Each candidate's audit verdict only weakly depends on which of those three dominates — the matrix-level gain ceiling differs by ~5 µs/cell at most across the three hypotheses.
3. Per Phase 1J task spec ("If unavailable: document and proceed with hypothesis-only analysis"), this is the documented branch.

## Implications for Phase 1J.3 / 1J.4 / 1J.5

Each candidate audit will state explicitly which sub-hypothesis (I-cache / LSU / key-const) it would close and by how much, given that none is yet PMU-measured. Phase 1J.B (or whichever sub-step ships first) MUST run the xctrace probe before committing implementation, to confirm the attack closes the dominant component rather than a 5 µs sliver.

## Suggested xctrace recipe for Phase 1J.B

```sh
# 1. Build luna release
cargo build --release -p luna-jit --example bench_a4_prime_token_bucket

# 2. Record CPU Counters trace (env-OFF + env-ON arms)
xctrace record --template "CPU Counters" \
  --output env-off.trace \
  --launch -- \
  target/release/examples/bench_a4_prime_token_bucket

LUNA_JIT_FIELD_IC=1 xctrace record --template "CPU Counters" \
  --output env-on.trace \
  --launch -- \
  target/release/examples/bench_a4_prime_token_bucket

# 3. Export per-counter samples
xctrace export --input env-off.trace --xpath \
  '//cpu-counters-data' > env-off.xml
xctrace export --input env-on.trace --xpath \
  '//cpu-counters-data' > env-on.xml

# 4. Diff per-PMU-event sample density between arms; the events
#    showing the largest env-ON delta are the dominant ~16 µs/cell
#    component.
```

Per-PMU-event mapping (Apple M-series, partial; verify against the
chip the probe runs on):

| PMC event | Component | What it implicates |
|---|---|---|
| `INST_BRANCH_MISPRED_NONSPEC` | Branch predictor | brif chain mispredict |
| `LSU_STALL_LD_DEP_*` | LSU pipeline | dependent-load chain through IC slot probe |
| `L1D_CACHE_MISS_NONSPEC` | I-cache / D-cache | inline IC body vs warmed helper body locality |
| `INST_BRANCH_TAKEN_NONSPEC` | Front-end | brif chain steady-state behaviour (not mispredict, just count) |
| `MEMORY_ORDER_VIOLATION` | LSU pipeline | rare; would indicate STLF stall (unlikely here) |

Without the probe, Phase 1J's candidate ranking treats the three
sub-hypotheses as equally weighted; Phase 1J.B's probe MUST resolve
which dominates before committing implementation.
