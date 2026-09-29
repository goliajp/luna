#!/usr/bin/env bash
# Same-runner perf gate for the redis_lua_shape criterion bench.
#
# Usage:
#   scripts/perf-gate.sh REF_BIN HEAD_BIN [THRESHOLD]
#
#   REF_BIN, HEAD_BIN: redis_lua_shape bench executables of the reference
#     commit and of HEAD, as built by
#     `cargo bench -p luna-jit --bench redis_lua_shape --no-run`
#   THRESHOLD: allowed slowdown factor (default 1.05)
#
# Environment:
#   PERF_ROUNDS  rounds of ref/head pairs per cell (default 3)
#   PERF_CPU     core both binaries are pinned to when taskset exists (default 1)
#   PERF_OUT     directory for criterion output (default: a fresh temp dir)
#
# Hosted runners drift over minutes, so measuring all of ref and then all of
# HEAD biases every cell the same way. Instead each cell is measured as an
# adjacent ref/head pair, the pair order alternates between cells and rounds,
# and a cell fails only when HEAD is slower than THRESHOLD in every round
# (ratio of the median per-iteration sample times). A commit body containing
# [perf-allow] skips the check.
set -euo pipefail

if [[ $# -lt 2 ]]; then
    sed -n '2,23p' "$0" >&2
    exit 2
fi
REF_BIN=$1
HEAD_BIN=$2
THRESHOLD=${3:-1.05}
ROUNDS=${PERF_ROUNDS:-3}
CPU=${PERF_CPU:-1}
OUT=${PERF_OUT:-$(mktemp -d)}
GROUP=redis_lua_shape

if git log -1 --pretty=%B 2>/dev/null | grep -qF '[perf-allow]'; then
    echo "perf-gate: [perf-allow] tag found in commit body — skipping regression check" >&2
    exit 0
fi

pin=()
if command -v taskset >/dev/null; then
    pin=(taskset -c "$CPU")
fi

list_cells() {
    "$1" --bench --list --format terse | sed -n "s|^$GROUP/\(.*\): benchmark\$|\1|p"
}
ref_cells=$(list_cells "$REF_BIN")
cells=()
for cell in $(list_cells "$HEAD_BIN"); do
    if grep -qxF "$cell" <<<"$ref_cells"; then
        cells+=("$cell")
    else
        echo "perf-gate: $cell has no reference measurement (new cell), not judged"
    fi
done
if [[ ${#cells[@]} -eq 0 ]]; then
    echo "perf-gate: no cells shared by the reference and HEAD" >&2
    exit 1
fi

run() {
    local side=$1 bin=$2 round=$3 cell=$4
    echo "::group::round $round $side $cell"
    CRITERION_HOME="$OUT/$side/r$round" ${pin[@]+"${pin[@]}"} "$bin" \
        --bench --exact --noplot "$GROUP/$cell"
    echo "::endgroup::"
}

for ((r = 1; r <= ROUNDS; r++)); do
    for i in "${!cells[@]}"; do
        cell=${cells[$i]}
        if (((r + i) % 2 == 0)); then
            run ref "$REF_BIN" "$r" "$cell"
            run head "$HEAD_BIN" "$r" "$cell"
        else
            run head "$HEAD_BIN" "$r" "$cell"
            run ref "$REF_BIN" "$r" "$cell"
        fi
    done
done

python3 "$(dirname "$0")/perf-gate-verdict.py" "$OUT" "$GROUP" "$ROUNDS" "$THRESHOLD" "${cells[@]}"
