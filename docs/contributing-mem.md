# Contributing — memory baseline workflow (Track MM)

luna ships a `mem_baseline` bench on `luna-core` that profiles five
representative workloads under [dhat][dhat] and writes a
machine-readable heap profile per workload, plus a one-line summary
table to stdout. This document covers running it locally, persisting
profiles, and re-baselining after a change.

[dhat]: https://docs.rs/dhat

## TL;DR

```bash
# run the five workloads, dump profiles to system tmp:
cargo bench --bench mem_baseline -p luna-core

# run them, pin profiles to the repo's checked-in baseline dir:
MM_DHAT_OUT=$(git rev-parse --show-toplevel)/.dev/baselines/mem-2026-06-25 \
  cargo bench --bench mem_baseline -p luna-core
```

The bench prints something like:

```
[mem_baseline]             cold_start  peak=    33068 B  steady=    31020 B  total=      44412 B  allocs=      435
[mem_baseline]             cold_start  profile  /.../cold_start.dhat.json
...
```

## The five workloads

| # | name                  | shape                                                      |
|---|-----------------------|------------------------------------------------------------|
| 1 | `cold_start`          | fresh `Vm::new` + one `eval("return 0")`                   |
| 2 | `repl_idle`           | 100 simple `eval("return N")`, REPL pattern                |
| 3 | `host_roots_churn`    | 1000 `pin_host` / `unpin` cycles                           |
| 4 | `alloc_collect`       | ~1M `local x = {}` + 10 explicit `collectgarbage("collect")` |
| 5 | `userdata_lifecycle`  | 200 finalizable tables (`__gc` metamethod) + 2 GC passes  |

Workloads pick patterns the v2.0 Track MM audit named as the
heap-budget attack surface. The fifth workload uses Lua-side
finalizable tables (not embedder Userdata) because luna-core's
public-facing Userdata API is too thin for a pure-Lua bench — the
`__gc` path it lowers to is what real Userdata allocations end up
hitting, so it is the closest in-tree proxy.

## Reading a workload row

```
peak       = high-water resident bytes during the workload window
steady     = resident bytes at end-of-window (after `body` returns but
             before the workload Vm is dropped, so `steady != 0`)
total      = cumulative bytes ever allocated (peak + reclaimed)
allocs     = total distinct allocation calls
peak/steady_blocks = same units in allocation-block count
```

`peak == steady` is the strongest "no leak / no growth" signal — see
`host_roots_churn` for the canonical example (1000 cycles, 0 growth).

## Inspecting a `.dhat.json` heap profile

The bench writes one `.dhat.json` per workload to `MM_DHAT_OUT`.
Three ways to read them:

1. **dhat web viewer** — open
   <https://nnethercote.github.io/dh_view/dh_view.html> and drop the
   `.dhat.json` in. Source-line attribution + flame view.
2. **`dhat-viewer` CLI** — `cargo install dhat-viewer`, then
   `dhat-viewer cold_start.dhat.json`. Same data, terminal UI.
3. **`jq` / quick scripts** — the JSON has `pps` (program points,
   one per alloc site) and `ftbl` (frame table). Sort `pps` by
   `tb` (total bytes) desc and look up frame indices in `ftbl`:

   ```bash
   python3 -c '
   import json, sys
   d = json.load(open(sys.argv[1]))
   for p in sorted(d["pps"], key=lambda x: -x["tb"])[:10]:
       frame = d["ftbl"][p["fs"][1]] if len(p["fs"]) > 1 else d["ftbl"][p["fs"][0]]
       print(f"tb={p[\"tb\"]:>10} blocks={p[\"tbk\"]:>6}  {frame}")
   ' cold_start.dhat.json
   ```

## Re-baselining after a change

If your change is expected to move memory numbers, refresh the
baseline:

```bash
# 1. run on the current branch + new MM_DHAT_OUT date dir
BASELINE_DIR=$(git rev-parse --show-toplevel)/.dev/baselines/mem-$(date +%Y-%m-%d)
mkdir -p "$BASELINE_DIR"
MM_DHAT_OUT="$BASELINE_DIR" cargo bench --bench mem_baseline -p luna-core | tee "$BASELINE_DIR/run.log"

# 2. write a summary.md alongside it noting:
#    - what changed
#    - delta per workload (before vs after, peak / steady / total)
#    - new top-N allocation sites if they shifted
#    - whether layout attacks 1-5 (or newly surfaced candidates) are
#      now justified by the data
```

The existing `.dev/baselines/mem-2026-06-25/summary.md` is the
template — copy its section layout for the new baseline.

## Why `dhat` is gated to `[dev-dependencies]`

`luna-core` ships the F1 zero-third-party-dependency contract:
`cargo tree -p luna-core --edges normal` must list exactly one crate
(luna-core itself). `dhat` is a heap profiler used only by the
`mem_baseline` bench, so it lives in `[dev-dependencies]`:

```toml
[dev-dependencies]
dhat = "0.3"

[[bench]]
name = "mem_baseline"
harness = false
```

Dev-deps never link into a downstream embedder's build, so the F1
contract is preserved. The CI `zero-dep` gate (in
`.github/workflows/ci.yml` and `.github/workflows/cargo-deny.yml`)
uses `cargo tree --edges normal` explicitly to enforce this — dev-deps
cannot regress the contract by accident.

## What's not yet wired (intentional, for the implementation phase)

- `Vm::heap_stats()` API surfacing per-arena bytes
- `Heap::exact_bytes()` (Vec/Box capacity tally on top of the
  intrusive heap byte count)
- `luna-heap-dump` CLI for runtime introspection
- `cargo bench mem` regression gate in CI

These all depend on layout decisions that follow the Track R IR
overhaul. The bench + checked-in baseline exist now so any future
work has ground truth to A/B against — see the audit summary in
`.dev/rfcs/v2.0-plan-state.md` (Track MM section) for the full
roadmap.
