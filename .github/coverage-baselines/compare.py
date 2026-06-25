#!/usr/bin/env python3
"""v2.0 Track CV-infra — coverage baseline diff.

Usage:
    compare.py <baseline.json> <current.json>

Exit codes:
    0  — no first-party crate dropped > 2pp in line coverage.
    1  — at least one regression > 2pp; details printed to stdout.

Warns (does not fail) when a crate sits below its audit budget. The
budget gates flip to hard-fail in a later phase once per-track CV
content fills bring each crate into budget.
"""

from __future__ import annotations

import json
import sys
from collections import defaultdict

# v2.0 Phase 0 Track CV audit budgets (per crate, line coverage):
BUDGETS = {
    "luna-core": 95.0,
    "luna-jit": 90.0,
    "luna-aot": 85.0,
    "luna-jit-derive": 85.0,
    "luna-runtime-helpers": 90.0,
}

# Regression band — drops > this many percentage points fail the gate.
REGRESSION_BAND_PP = 2.0


def aggregate_per_crate(json_path: str) -> dict[str, dict[str, float]]:
    with open(json_path) as f:
        data = json.load(f)

    crates: dict[str, dict[str, float]] = defaultdict(
        lambda: {
            "lines_covered": 0,
            "lines_total": 0,
            "regions_covered": 0,
            "regions_total": 0,
        }
    )

    for fdata in data["data"][0]["files"]:
        parts = fdata["filename"].split("/")
        if "crates" not in parts:
            continue
        cn = parts[parts.index("crates") + 1]
        s = fdata["summary"]
        crates[cn]["lines_covered"] += s["lines"]["covered"]
        crates[cn]["lines_total"] += s["lines"]["count"]
        crates[cn]["regions_covered"] += s["regions"]["covered"]
        crates[cn]["regions_total"] += s["regions"]["count"]

    result: dict[str, dict[str, float]] = {}
    for cn, v in crates.items():
        lp = (
            100.0 * v["lines_covered"] / v["lines_total"]
            if v["lines_total"]
            else 0.0
        )
        rp = (
            100.0 * v["regions_covered"] / v["regions_total"]
            if v["regions_total"]
            else 0.0
        )
        result[cn] = {"lines_pct": lp, "regions_pct": rp}
    return result


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2

    baseline = aggregate_per_crate(sys.argv[1])
    current = aggregate_per_crate(sys.argv[2])

    failed = False

    print("\n=== Per-crate coverage diff ===\n")
    print(
        f"{'crate':25s} {'baseline':>10s} {'current':>10s} "
        f"{'delta':>8s} {'budget':>8s} {'status':>15s}"
    )
    print("-" * 80)

    for cn in sorted(set(baseline) | set(current)):
        b = baseline.get(cn, {"lines_pct": 0.0})["lines_pct"]
        c = current.get(cn, {"lines_pct": 0.0})["lines_pct"]
        delta = c - b
        budget = BUDGETS.get(cn)

        status_parts: list[str] = []
        if delta < -REGRESSION_BAND_PP:
            status_parts.append("FAIL-regression")
            failed = True
        if budget is not None and c < budget:
            status_parts.append("warn-below-budget")
        if not status_parts:
            status_parts.append("ok")
        status = ",".join(status_parts)

        budget_str = f"{budget:.0f}%" if budget is not None else "n/a"
        print(
            f"{cn:25s} {b:9.2f}% {c:9.2f}% {delta:+7.2f}pp "
            f"{budget_str:>8s} {status:>15s}"
        )

    print()
    if failed:
        print(
            "::error::At least one crate dropped > "
            f"{REGRESSION_BAND_PP:.1f}pp in line coverage vs baseline."
        )
        return 1
    print("Coverage gate: PASS (no regressions > "
          f"{REGRESSION_BAND_PP:.1f}pp).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
