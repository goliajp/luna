#!/usr/bin/env bash
# Same-runner perf gate for the redis_lua_shape criterion bench.
#
# Usage:
#   scripts/perf-gate.sh measure REF_BIN HEAD_BIN OUT
#   scripts/perf-gate.sh verdict OUT...
#   scripts/perf-gate.sh REF_BIN HEAD_BIN          (measure into a temp dir, then verdict)
#
#   REF_BIN, HEAD_BIN: redis_lua_shape bench executables of the reference
#     commit and of HEAD, as built by
#     `cargo bench -p luna-jit --bench redis_lua_shape --no-run`
#   OUT: one directory per measuring job; the verdict pools several of them
#
# Environment:
#   PERF_ROUNDS     rounds of ref/head pairs per cell in one measure (default 3)
#   PERF_CPU        core both binaries are pinned to when taskset exists (default 1)
#   PERF_THRESHOLD  passed to the verdict, see perf-gate-verdict.py
#
# Hosted runners drift over minutes, so measuring all of ref and then all of
# HEAD biases every cell the same way. Instead each cell is measured as an
# adjacent ref/head pair and the pair order alternates between cells and
# rounds. The ref/head ratio of the same two binaries also differs from one
# runner to the next by a few percent, which more rounds on one runner cannot
# average out; CI therefore runs `measure` on several runners and pools them
# in one `verdict`. A HEAD commit message containing [perf-allow] skips the check.
set -euo pipefail

GROUP=redis_lua_shape
here=$(dirname "$0")

usage() {
    sed -n '2,25p' "$0" >&2
    exit 2
}

perf_allowed() {
    if git log -1 --pretty=%B 2>/dev/null | grep -qF '[perf-allow]'; then
        echo "perf-gate: [perf-allow] tag found in the commit message — skipping regression check" >&2
        return 0
    fi
    return 1
}

list_cells() {
    "$1" --bench --list --format terse | sed -n "s|^$GROUP/\(.*\): benchmark\$|\1|p"
}

measure() {
    local ref_bin=$1 head_bin=$2 out=$3
    local rounds=${PERF_ROUNDS:-3} cpu=${PERF_CPU:-1}
    local pin=()
    if command -v taskset >/dev/null; then
        pin=(taskset -c "$cpu")
    fi

    local ref_cells cells=() cell
    ref_cells=$(list_cells "$ref_bin")
    for cell in $(list_cells "$head_bin"); do
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
    mkdir -p "$out"
    printf '%s\n' "${cells[@]}" >"$out/cells"

    run() {
        local side=$1 bin=$2 round=$3 cell=$4
        echo "::group::round $round $side $cell"
        CRITERION_HOME="$out/$side/r$round" ${pin[@]+"${pin[@]}"} "$bin" \
            --bench --exact --noplot "$GROUP/$cell"
        echo "::endgroup::"
    }

    local r i
    for ((r = 1; r <= rounds; r++)); do
        for i in "${!cells[@]}"; do
            cell=${cells[$i]}
            if (((r + i) % 2 == 0)); then
                run ref "$ref_bin" "$r" "$cell"
                run head "$head_bin" "$r" "$cell"
            else
                run head "$head_bin" "$r" "$cell"
                run ref "$ref_bin" "$r" "$cell"
            fi
        done
    done
}

verdict() {
    python3 "$here/perf-gate-verdict.py" ${PERF_THRESHOLD:+--threshold "$PERF_THRESHOLD"} "$@"
}

case ${1:-} in
measure)
    [[ $# -eq 4 ]] || usage
    perf_allowed && exit 0
    measure "$2" "$3" "$4"
    ;;
verdict)
    [[ $# -ge 2 ]] || usage
    perf_allowed && exit 0
    shift
    verdict "$@"
    ;;
*)
    [[ $# -eq 2 ]] || usage
    perf_allowed && exit 0
    out=$(mktemp -d)
    measure "$1" "$2" "$out"
    verdict "$out"
    ;;
esac
