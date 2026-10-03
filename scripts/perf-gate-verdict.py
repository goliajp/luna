#!/usr/bin/env python3
"""Judge the rounds written by `perf-gate.sh measure`.

Usage: perf-gate-verdict.py [--threshold T] OUT...

Each OUT is one measuring job: OUT/cells lists the judged cells and
OUT/{ref,head}/r<N>/redis_lua_shape/<cell>/new/sample.json holds the rounds.

A round's ratio is median(head ns/iter) / median(ref ns/iter). A job's
ratio is the median of its rounds, which discards a round disturbed by the
runner. The cell's estimate is the geometric mean of the job ratios: the
ref/head ratio of the same two binaries differs between runners by a few
percent, so the runners are the unit that has to be averaged. A cell fails
when its estimate exceeds T.

The default T = 1.0315 is derived for 7 jobs x 3 rounds from hosted-runner
measurements: the job-to-job spread of a cell's ratio is taken as 0.021
(worst seen) and the spread of a 3-round median within a job as 0.017
(worst cell), giving a standard error of 0.0102 on ln(estimate). With that,
an unchanged commit fails with probability <= 0.01 summed over 6 cells, and
a 5% slowdown in any one cell fails with probability >= 0.95. The threshold
only holds for that layout; change it together with the job and round
counts in perf.yml.
"""
import argparse
import json
import math
import os
import statistics

GROUP = "redis_lua_shape"

ap = argparse.ArgumentParser()
ap.add_argument("--threshold", type=float, default=1.0315)
ap.add_argument("outs", nargs="+")
args = ap.parse_args()


def rounds_of(out):
    return sorted(
        int(d[1:]) for d in os.listdir(os.path.join(out, "ref")) if d.startswith("r")
    )


def median_ns(out, side, r, cell):
    path = os.path.join(out, side, f"r{r}", GROUP, cell, "new", "sample.json")
    with open(path) as f:
        s = json.load(f)
    return statistics.median(t / n for t, n in zip(s["times"], s["iters"]))


cells = None
for out in args.outs:
    with open(os.path.join(out, "cells")) as f:
        these = f.read().split()
    if cells is not None and these != cells:
        raise SystemExit(f"perf-gate: {out} measured {these}, expected {cells}")
    cells = these

jobs = [(out, rounds_of(out)) for out in args.outs]
fail = False
print(
    f"perf-gate: {len(jobs)} jobs x {'/'.join(str(len(r)) for _, r in jobs)} rounds, "
    f"fails when the geometric mean of the per-job median ratios exceeds {args.threshold:.4f}x"
)
for cell in cells:
    per_job = []
    for out, rounds in jobs:
        ratios = [
            median_ns(out, "head", r, cell) / median_ns(out, "ref", r, cell) for r in rounds
        ]
        per_job.append(statistics.median(ratios))
        print(f"  {cell:<22} {os.path.basename(out.rstrip('/')):<12} "
              + " ".join(f"{x:.3f}" for x in ratios)
              + f"  median {per_job[-1]:.3f}")
    est = math.exp(statistics.fmean(math.log(x) for x in per_job))
    status = "OK"
    if est > args.threshold:
        status = "REGRESS"
        fail = True
    print(f"  {cell:<22} estimate {est:.3f}x  {status}")

if fail:
    print("perf-gate: FAIL — a cell is slower than the threshold", flush=True)
    raise SystemExit(1)
print("perf-gate: PASS")
