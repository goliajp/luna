#!/usr/bin/env python3
"""Extract a perf-gate baseline JSON from a criterion output dir.

v2.14 bench-infra: the perf-gate now benches a pinned REFERENCE
commit on the SAME runner as HEAD and diffs against that, instead
of a fixed-ns baseline file — GHA hosted runners are heterogeneous
(observed 0.51x-1.09x spread for identical code across runs), so
absolute nanoseconds are not comparable across runs, only within
one.

Usage: criterion-extract.py <criterion-group-dir> <label> > out.json
"""
import json
import os
import sys

crit_dir, label = sys.argv[1], sys.argv[2]
cells = {}
for cell in sorted(os.listdir(crit_dir)):
    est = os.path.join(crit_dir, cell, "new", "estimates.json")
    if not os.path.isfile(est):
        continue
    with open(est) as f:
        mean = json.load(f)["mean"]
    cells[cell] = {
        "mean_ns": mean["point_estimate"],
        "mean_low_ns": mean["confidence_interval"]["lower_bound"],
        "mean_high_ns": mean["confidence_interval"]["upper_bound"],
    }
if not cells:
    print(f"criterion-extract: no cells under {crit_dir}", file=sys.stderr)
    sys.exit(1)
json.dump(
    {"group": os.path.basename(crit_dir.rstrip("/")), "platform": label, "cells": cells},
    sys.stdout,
    indent=2,
)
print()
