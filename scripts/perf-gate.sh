#!/usr/bin/env bash
# Luna per-PR perf-gate (v2.6 Track B)
#
# Usage:
#   scripts/perf-gate.sh [BASELINE_JSON] [THRESHOLD]
#
# Args:
#   BASELINE_JSON: path to baseline file
#     (default: .dev/perf-baselines/v2.6.0-macos-arm64.json)
#   THRESHOLD: regression factor allowed (default: 1.05 = 5%)
#
# Behavior:
#   - Reads criterion estimates at
#     target/criterion/redis_lua_shape/<cell>/new/estimates.json
#   - Diffs mean_ns against baseline mean_ns
#   - Exit 1 if any cell regresses past THRESHOLD
#   - Exit 0 if all cells within threshold
#   - Honors [perf-allow] in commit body to bypass (exit 0 with warning)
#
# Expects: `cargo bench -p luna-jit --bench redis_lua_shape -- \
#   --measurement-time 8 --warm-up-time 2` was run before this script.
set -euo pipefail

# v2.7 B.2: auto-select baseline by runner OS. CI sets
# $RUNNER_OS (Linux / macOS / Windows); local dev defaults to
# macOS arm64 baseline.
if [[ -z "${1:-}" ]]; then
    case "${RUNNER_OS:-macOS}" in
        Linux)
            BASELINE="crates/luna-jit/perf-baselines/v2.7.0-ubuntu-x86_64.json"
            ;;
        macOS|*)
            BASELINE="crates/luna-jit/perf-baselines/v2.6.0-macos-arm64.json"
            ;;
    esac
else
    BASELINE="$1"
fi
THRESHOLD="${2:-1.05}"

if [[ ! -f "$BASELINE" ]]; then
    echo "perf-gate: baseline file $BASELINE missing — recording new baseline expected (no diff)" >&2
    exit 0
fi

# perf-allow bypass: commit message body contains literal [perf-allow]
if git log -1 --pretty=%B 2>/dev/null | grep -qF '[perf-allow]'; then
    echo "perf-gate: [perf-allow] tag found in commit body — skipping regression check" >&2
    exit 0
fi

CRIT_DIR="target/criterion/redis_lua_shape"
if [[ ! -d "$CRIT_DIR" ]]; then
    echo "perf-gate: $CRIT_DIR missing — run `cargo bench -p luna-jit --bench redis_lua_shape` first" >&2
    exit 1
fi

python3 - "$BASELINE" "$THRESHOLD" "$CRIT_DIR" <<'PYEOF'
import json, os, sys

baseline_path, threshold, crit_dir = sys.argv[1], float(sys.argv[2]), sys.argv[3]
baseline = json.load(open(baseline_path))
fail = False
print(f"perf-gate: baseline={baseline_path} threshold={threshold:.3f}x")
print(f"  {'cell':<24} {'baseline_ns':>14} {'current_ns':>14} {'ratio':>8}  status")
for cell, base in baseline["cells"].items():
    est_path = os.path.join(crit_dir, cell, "new", "estimates.json")
    if not os.path.isfile(est_path):
        print(f"  {cell:<24} {'MISSING':>14}")
        fail = True
        continue
    cur = json.load(open(est_path))["mean"]["point_estimate"]
    base_ns = base["mean_ns"]
    ratio = cur / base_ns
    status = "OK"
    if ratio > threshold:
        status = "REGRESS"
        fail = True
    print(f"  {cell:<24} {base_ns:>14.0f} {cur:>14.0f} {ratio:>7.3f}x  {status}")

if fail:
    print("perf-gate: FAIL — at least one cell regressed past threshold", file=sys.stderr)
    sys.exit(1)
print("perf-gate: PASS — all cells within threshold")
PYEOF
