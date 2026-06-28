# Contributing — running coverage locally

luna's CI gates per-crate **line coverage** against a committed baseline
(`.github/coverage-baselines/coverage-2026-06-25.json`). A PR fails if
any first-party crate drops more than 2 percentage points from baseline.

This page covers running the same workflow on your machine so you can
see the regression before pushing.

## One-time install

```
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov
```

`cargo-llvm-cov` works on stable rustc. Branch coverage would require
nightly + `-Z coverage-options=branch`; the CI gate operates on
**lines** (with regions reported as a branch-style proxy) so a stable
toolchain is enough.

## Run the same shape as CI

```
cargo llvm-cov --workspace --lib --json \
  --output-path coverage-current.json --summary-only
```

This produces a JSON report identical in shape to the committed
baseline. To see the per-crate breakdown directly:

```
cargo llvm-cov report --summary-only
```

## Diff vs the committed baseline

```
python3 .github/coverage-baselines/compare.py \
  .github/coverage-baselines/coverage-2026-06-25.json \
  coverage-current.json
```

Output is per-crate `baseline / current / delta / budget / status`. The
script exits non-zero on a `> 2pp` drop in any crate, matching the CI
gate exactly.

## Updating the baseline

Don't update the baseline in the same PR that causes the regression —
that defeats the gate. To intentionally re-baseline (e.g. after a
landed CV-content sprint that genuinely raised coverage):

1. Verify the new coverage on `develop` post-merge:
   `cargo llvm-cov --workspace --lib --json --output-path /tmp/new.json --summary-only`
2. File a dedicated re-baseline PR:
   ```
   cp /tmp/new.json .github/coverage-baselines/coverage-<today>.json
   # update the path in .github/workflows/coverage.yml and compare.py CLI
   # in PR description: link the merged CV-content commits + their delta
   ```
3. Get a reviewer to confirm the new baseline is at least as covered as
   the old one for every crate.

## Per-crate audit budgets

| crate                | line coverage budget |
|----------------------|----------------------|
| luna-core            | ≥ 95% (stone)        |
| luna-jit             | ≥ 90% (steel)        |
| luna-aot             | ≥ 85% (cement)       |
| luna-runtime-helpers | ≥ 90%                |
| luna-jit-derive      | ≥ 85%                |

These budgets come from the Phase 0 Track CV audit. Today's baseline
(2026-06-25) is well below for several crates — the CV-content
implementation sprints close the gap per crate. The CI gate currently
**warns** when below budget; it flips to **hard-fail** once a per-crate
budget is reached at least once (locks in the achievement against
regression).

## Running fuzz harnesses locally

See `crates/luna-fuzz/README.md`.
