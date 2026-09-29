#!/usr/bin/env python3
"""Judge the rounds written by perf-gate.sh.

Usage: perf-gate-verdict.py OUT GROUP ROUNDS THRESHOLD CELL...

Reads OUT/{ref,head}/r<N>/GROUP/<cell>/new/sample.json. For each round the
ratio is median(head ns/iter) / median(ref ns/iter); the median resists the
outlier samples a shared runner produces. A cell fails when every round's
ratio exceeds THRESHOLD, so one disturbed round cannot fail the gate while a
real slowdown shows up in all of them.
"""
import json
import os
import statistics
import sys

out, group, rounds, threshold = sys.argv[1], sys.argv[2], int(sys.argv[3]), float(sys.argv[4])
cells = sys.argv[5:]


def median_ns(side, r, cell):
    path = os.path.join(out, side, f"r{r}", group, cell, "new", "sample.json")
    with open(path) as f:
        s = json.load(f)
    return statistics.median(t / n for t, n in zip(s["times"], s["iters"]))


fail = False
cols = " ".join(f"{'r' + str(r):>7}" for r in range(1, rounds + 1))
print(f"perf-gate: {rounds} interleaved rounds, threshold={threshold:.3f}x (fails only if every round is over)")
print(f"  {'cell':<22} {'ref_ns':>12} {'head_ns':>12} {cols} {'min':>7}  status")
for cell in cells:
    ref = [median_ns("ref", r, cell) for r in range(1, rounds + 1)]
    head = [median_ns("head", r, cell) for r in range(1, rounds + 1)]
    ratios = [h / b for h, b in zip(head, ref)]
    lo = min(ratios)
    status = "OK"
    if lo > threshold:
        status = "REGRESS"
        fail = True
    per_round = " ".join(f"{x:>6.3f}x" for x in ratios)
    print(
        f"  {cell:<22} {statistics.median(ref):>12.0f} {statistics.median(head):>12.0f} "
        f"{per_round} {lo:>6.3f}x  {status}"
    )

if fail:
    print("perf-gate: FAIL — a cell is over threshold in every round", file=sys.stderr)
    sys.exit(1)
print("perf-gate: PASS")
