# Coverage baseline — 2026-06-25 (Track CV-infra)

Baseline JSON: `coverage-2026-06-25.json` (full per-file detail, LLVM
`--json --summary-only` shape from `cargo llvm-cov 0.8.7`).

> **Note on file location.** Task spec asked for the baseline under
> `.dev/baselines/`, but `.dev/` is `.gitignore`'d (`.gitignore` line 7).
> A baseline that CI must read against has to live in a *tracked* path,
> so the canonical copy lives here in `.github/coverage-baselines/`. The
> `.dev/baselines/` copy still exists locally as a personal reference.
> Surfaced in the implementation report; flagging this for the next
> session to confirm the location convention.

Command used:

```
cargo llvm-cov --workspace --lib --json \
  --output-path .github/coverage-baselines/coverage-2026-06-25.json \
  --summary-only
```

Run shape: `--lib` only (workspace lib tests). `--all-targets` would
also exercise `bin/example/test`, but the audit budgets are explicitly
on **library code** — the bin/example coverage is a different
conversation (many bins are dogfood/smoke harnesses; `luna-aot` and
`luna-jit-derive` integration tests sit there too — see gap #2 / #3).

## Per-crate aggregate vs audit budget

| crate                  | lines  | budget | gap     | regions | functions | branches |
|------------------------|--------|--------|---------|---------|-----------|----------|
| luna-core              | 34.33% | ≥95%   | -60.67% | 32.90%  | 42.04%    | n/a*     |
| luna-jit               | 61.75% | ≥90%   | -28.25% | 61.31%  | 67.38%    | n/a*     |
| luna-aot               |  0.00% | ≥85%   | -85.00% |  0.00%  |  0.00%    | n/a*     |
| luna-jit-derive        |  0.00% | ≥85%   | -85.00% |  0.00%  |  0.00%    | n/a*     |
| luna-runtime-helpers   |  0.00% | ≥90%   | -90.00% |  0.00%  |  0.00%    | n/a*     |

\* Branch coverage requires nightly rustc + `-Z coverage-options=branch`.
LLVM's stable instrumentation only emits region-level data; the
`branches` column reads 0/0 across the board on stable. CI gate uses
**regions** as the proxy for branch-style coverage (Phase 2 audit
decision — see also `.dev/rfcs/v2.0-audit-CV.md`'s self-hosted JSON
baseline rationale).

## Top-3 coverage gaps vs budget (priority for implementation phase)

1. **luna-runtime-helpers @ 0%** — staticlib whose `#[no_mangle]`
   symbols are exercised **only** from AOT-built binaries at runtime,
   not from cargo-test. `lib.rs: 548 lines / 0 covered`. Per-track fill
   needs a harness that builds a tiny AOT binary in-process and runs it
   (the existing `crates/luna-aot` smoke test pattern). **Largest
   single gap in absolute lines.**

2. **luna-aot @ 0%** — bin-only crate with no lib tests today. 17
   tests exist but live in `tests/` integration shape and don't surface
   via `--lib`. Implementation phase needs `cargo llvm-cov --workspace
   --all-targets` once we accept that bins/examples are in budget; OR
   move the integration tests into the lib's `#[cfg(test)] mod tests`.

3. **luna-jit-derive @ 0%** — proc-macro crate. Coverage of proc-macros
   is awkward (compile-time execution). Audit budget of 85% assumes
   `cargo expand` + golden-file tests in `tests/`; today there are 8
   tests that exist but again don't surface via `--lib`. Same shape as
   luna-aot — needs `--tests` inclusion or layout move.

## luna-core hottest under-covered files (top 10)

(sorted by uncovered lines × audit weight; see baseline JSON for the
full list)

| file                                       | line cov |
|--------------------------------------------|----------|
| crates/luna-core/src/vm/lib_debug.rs       |  8.24%   |
| crates/luna-core/src/vm/lib_table.rs       |  7.17%   |
| crates/luna-core/src/vm/lib_string.rs      |  9.90%   |
| crates/luna-core/src/vm/lib_os_io.rs       | 13.12%   |
| crates/luna-core/src/vm/builtins.rs        | 20.17%   |
| crates/luna-core/src/vm/dump/luna.rs       |  0.00%   |
| crates/luna-core/src/vm/dump/puc/puc_51.rs |  5.16%   |
| crates/luna-core/src/vm/dump/puc/puc_52.rs | 10.03%   |
| crates/luna-core/src/vm/userdata_trait.rs  |  0.00%   |
| crates/luna-core/src/vm/host_roots.rs      |  0.00%   |

Reading: most uncovered code is library code (`lib_*`, `dump/puc/*`)
whose tests live as `.lua` scripts under `tests/official/` and run via
the `luna` binary, not as `cargo test --lib`. Per-track CV-content
fills in v2.0 implementation phase need to either:

* port a representative subset of `.lua` script tests to inline
  `#[cfg(test)] mod tests` blocks driving `Vm::eval` (preferred — keeps
  the assertion in-source),
* OR include `--bins --tests` in the coverage run (changes the budget
  basis — separate audit conversation).

## CI gate semantics

The Phase 2 `coverage.yml` workflow:

* compares **per-crate line coverage** in fresh runs vs this baseline
  JSON (region coverage tracked too but gated only on lines for
  stability),
* fails if any crate drops > 2 percentage points (regression band),
* warns (does not fail) if any crate is below its audit budget — the
  budget gates only flip to hard-fail once per-track content fills
  bring each crate into its budget.

This shape lets Phase 2 ship the **infrastructure** without blocking
on the multi-week per-track content work.
