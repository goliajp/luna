# Contributing — re-running the disk + binary-size baseline

This is the reproduction guide for the workspace's disk-size baseline
measurements (Track DS). Run these commands on macOS aarch64 (or
adapt as noted) to produce numbers comparable to the committed
baseline snapshot. Sister doc: [`binary-size.md`](binary-size.md)
covers the historical `cargo bloat` snapshot for the `luna` runner
CLI; this file covers the broader workspace + AOT-output sizes.

## What gets measured

1. **Per-crate publish size** — `cargo publish --dry-run` for each of
   the five workspace crates (`luna-core`, `luna-jit-derive`,
   `luna-jit`, `luna-runtime-helpers`, `luna-aot`). Captures both raw
   tarball bytes and gzip-compressed bytes — the latter is what
   crates.io serves.
2. **AOT output binary size** — three representative scripts compiled
   through `luna-aot compile`, both dev and release profiles, both
   raw and `strip`ped. The three scripts are `hello.lua` (1 LOC),
   `fib.lua` (5 LOC recursive fib(28)), and `production_like.lua`
   (~1.5k LOC synthetic handler-dispatch workload).
3. **Section breakdown** — `size -m` against the release build of
   `production_like.lua` to identify which Mach-O sections dominate
   (`__TEXT/__text`, `__eh_frame`, `__LINKEDIT`, the embedded
   `.luna.bytecode` segment, etc.).
4. **Runtime-helpers library size** — both staticlib (`.a`) and
   rlib (`.rlib`), dev and release. The staticlib is what AOT
   binaries statically link against.

## One-shot reproduce (~3 min on warm cache, ~10 min cold)

```bash
# 0. Pre-reqs: clean tree, no uncommitted target/ junk.
cargo clean

# 1. Per-crate publish sizes (5 crates).
for c in luna-core luna-jit-derive luna-jit luna-runtime-helpers luna-aot; do
  echo "=== $c ==="
  cargo publish --dry-run -p "$c" 2>&1 | grep -E "Packaged|warning"
done

# 2. Build both luna-aot CLI and luna-runtime-helpers staticlib
# (dev + release).
cargo build -p luna-aot                 # dev
cargo build --release -p luna-aot       # release
cargo build -p luna-runtime-helpers     # dev (staticlib + rlib)
cargo build --release -p luna-runtime-helpers
ls -la target/{debug,release}/libluna_runtime_helpers.{a,rlib}

# 3. Author three representative scripts. hello.lua and fib.lua are
# trivial; production_like.lua is generated. See "Generating
# production_like.lua" below.

# 4. Compile each script with each profile, with the matching helper.
mkdir -p /tmp/luna-ds-out && cd /tmp/luna-ds-out
WORKTREE="$(cd - && pwd)"  # repo root
for prof in dev release; do
  AOT_BIN="$WORKTREE/target/${prof/dev/debug}/luna-aot"
  HELPER="$WORKTREE/target/${prof/dev/debug}/libluna_runtime_helpers.a"
  [ "$prof" = "release" ] && HELPER="$WORKTREE/target/release/libluna_runtime_helpers.a"
  for s in hello fib production_like; do
    LUNA_AOT_RUNTIME_HELPERS_STATICLIB="$HELPER" \
      "$AOT_BIN" compile "/tmp/luna-ds-scripts/$s.lua" -o "${s}_${prof}"
  done
done

# 5. Strip release binaries.
for s in hello fib production_like; do
  cp "${s}_release" "${s}_release_stripped"
  strip "${s}_release_stripped"
done
ls -la

# 6. Section breakdown (macOS only — Linux: use `objdump -h`).
size -m production_like_release

# 7. Runtime-helpers .a + .rlib byte sizes already captured above.
```

## Generating `production_like.lua`

The script is a synthetic ~1.5k-LOC handler-dispatch workload:
80 closures of identical shape (`handler_NN(req, ctx)`) plus a
router. It exercises closure/upvalue allocation, table layout, and
self-recursion — representative of a production embedder's Lua
workload, but deterministic. The exact generator script is committed
under `.dev/baselines/disk-2026-06-25/` (gitignored), but the recipe
is small enough to re-derive:

```python
import sys
print("local M = {}")
for i in range(80):
    print(f"""
local function handler_{i}(req, ctx)
  local user = req.user or {{name = "anon_{i}"}}
  local payload = {{}}
  for k, v in pairs(req.params or {{}}) do
    payload[k] = tostring(v) .. "_seen_{i}"
  end
  if ctx.depth and ctx.depth > 0 then
    return handler_{i}(req, {{depth = ctx.depth - 1}})
  end
  local result = {{}}
  for j = 1, 10 do
    result[#result+1] = {{idx = j, kind = "k_{i}", user = user.name}}
  end
  return {{status = 200, body = result}}
end
M["handler_{i}"] = handler_{i}""")
print("""
local function dispatch(route, req)
  local h = M[route]
  if h then return h(req, {depth = 0}) end
  return {status = 404}
end
for i = 1, 5 do dispatch("handler_" .. i, {user = {name = "tester"}, params = {[tostring(i)] = i * 100}}) end
print("done")
""")
```

Save its output as `production_like.lua` next to `hello.lua` and
`fib.lua` (`print(fib(28))` 5-liner) under `/tmp/luna-ds-scripts/`.

## Comparing against the committed baseline

The committed snapshot lives under `.dev/baselines/disk-<date>/`
(gitignored locally; future on-tree archive will live at
`docs/baselines/disk-<date>/` once we promote it). Each snapshot
contains:

- `crate-sizes.md`
- `aot-binary-sizes.md`
- `macho-sections.md`
- `runtime-helpers-sizes.md`
- `summary.md`

To check for drift after a sprint, re-run the steps above and diff
the resulting numbers against the latest snapshot. The Track DS
gate (TBD, lands during the v2.0 implementation phase) will fail CI
when measured sizes exceed budget targets enumerated in the
`summary.md` of the most recent snapshot.

### Drift thresholds (informational)

- **Per-crate compressed package**: flag if >5% from baseline. Most
  changes register as <1%; >5% means a real new dep landed or a
  bytes-heavy resource was added.
- **AOT release stripped**: flag if >5% from baseline for the same
  script. Per-LOC overhead is ~56 bytes stripped, so a 1k-LOC delta
  in `production_like.lua` is expected to move the number by ~56 KiB
  (~1.2%); larger moves point at runtime-side regressions.
- **`__text` section**: flag if >10%. Bigger noise floor here because
  cranelift trace mcode varies per build.

## Cross-platform notes

- **Linux**: replace `size -m` with `objdump -h <binary>` (or
  `readelf -SW`); section names are `.text` / `.eh_frame` /
  `.rodata` etc. The `strip` invocation is the same. The Stage-6
  Alpine smoke test (`docs/architecture.md` Phase AOT Stage 6) is a
  good cross-check that the staticlib link path still works.
- **Windows (MinGW)**: replace `size -m` with `objdump -h` (mingw
  binutils); PE section names are `.text` / `.pdata` / `.xdata`
  (the latter two are unwind tables analogous to `__eh_frame`).
- **Cross-compile**: pass `--target` to both
  `cargo build -p luna-runtime-helpers --target ...` and
  `luna-aot compile --target ...`. See
  [`docs/architecture.md`](architecture.md) Phase AOT Stage 5 for
  the supported triple matrix.

## Why no `cargo bloat` here?

`cargo bloat` (used by [`binary-size.md`](binary-size.md)) operates on
a single binary's symbol table and tells you "which crate's code
dominates `.text`". It does not measure:

- crate publish package sizes,
- per-section bytes (eh_frame, LINKEDIT, embedded data segments),
- staticlib bytes vs. final-binary bytes (post dead-strip).

The two tools are complementary: re-run `cargo bloat` to see *which
crates* contribute to `.text` growth; re-run the commands in this
doc to track absolute disk + section bytes across a sprint.
