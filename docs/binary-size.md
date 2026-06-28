# Binary size

Binary size budgeting + reproduction reference. Numbers refresh on
crate-boundary changes; the canonical baselines live at
`.dev/baselines/disk-2026-06-25/` (gitignored, reproducible via
[`contributing-disk.md`](contributing-disk.md)).

For deploy-side decision criteria, see [`deploy.md`](deploy.md) §2
(packaging shapes). For the AOT binary's section composition
specifically, see [`aot.md`](aot.md) §4.

---

## 1. Per-crate package sizes (v1.3 ship)

`cargo publish --dry-run` measurements, 2026-06-25:

| Crate | Files | Raw | Compressed |
|---|---:|---:|---:|
| `luna-core` | 285 | 4.4 MiB | 1.6 MiB |
| `luna-jit` | 175 | 1.2 MiB | 286 KiB |
| `luna-aot` | 47 | 268 KiB | 76 KiB |
| `luna-runtime-helpers` | 32 | 107 KiB | 31 KiB |
| `luna-jit-derive` | 6 | 28 KiB | 10 KiB |

`luna-core`'s 4.4 MiB raw is dominated by the PUC test corpora bundled
under `crates/luna-core/tests/official/` — the v2.0 Track DS reduction
candidate "PUC corpora exclude from luna-core publish" (audit-listed)
would shrink the published artifact ~60% (-2.6 MiB raw / -1.0 MiB
compressed) without touching runtime behavior.

## 2. AOT output binary sizes

Three representative inputs measured 2026-06-25 on macOS aarch64:

| Script | Source | Dev | Release | Release-stripped |
|---|---|---:|---:|---:|
| `hello.lua` | 1 line | 12.4 MiB | 6.0 MiB | **4.5 MiB** |
| `fib.lua` | fib_28 | 12.4 MiB | 6.0 MiB | **4.5 MiB** |
| `production_like.lua` | ~1.5k LOC | 12.5 MiB | 6.1 MiB | **4.6 MiB** |

Source-size linearity is weak below ~10 KiB of input: the **runtime
floor** dominates. Beyond that, `.luna.bytecode` grows ~1 byte per
emitted opcode (plus constants). The 1.5k-LOC `production_like.lua`
only differs from `hello.lua` by ~82 KiB stripped.

Section breakdown for `production_like.lua` release-stripped:

| Section | Size | Notes |
|---|---:|---|
| `__TEXT/__text` | 3.4 MiB | Static interp + GC + stdlib + Cranelift-emitted trace mcode |
| `__TEXT/__cstring` | ~250 KiB | Symbol strings + diagnostic message literals |
| `__TEXT/__eh_frame` + `__unwind_info` | 413 KiB | Unwind tables — `panic="abort"` profile can shrink this |
| `.luna.bytecode` | 96 KiB | The embedded chunk's bytecode dump |
| `__DATA/__data` | ~150 KiB | Per-trace AOT metadata tables + global statics |
| `__LINKEDIT` | 272 KiB | Symbol table + load commands (post-strip) |

Full breakdown at
`.dev/baselines/disk-2026-06-25/macho-sections.md`.

## 3. luna-runtime-helpers

The staticlib that the AOT-produced binary links against:

| Artifact | Profile | Size |
|---|---|---:|
| `libluna_runtime_helpers.a` | release | 40.7 MiB |
| `libluna_runtime_helpers.a` | dev | 97 MiB |
| `libluna_runtime_helpers.rlib` | release | 285 KiB |
| `libluna_runtime_helpers.rlib` | dev | 787 KiB |

The release staticlib is large but the **link step's linker
deadstrip pass** discards everything the AOT binary doesn't call,
which is why the produced binary is ~4.5 MiB instead of ~40 MiB.

## 4. Reduction candidates (v2.0 Track DS)

Proposed budgets + lever feasibility, from
`.dev/rfcs/v2.0-plan-state.md` §Track DS summary + audit:

| Lever | Estimated effect | Feasibility | Status |
|---|---|---|---|
| PUC corpora exclude from `luna-core` publish | -60% raw / -1 MiB compressed | M (test path audit) | v2.0 Track DS implementation |
| `panic = "abort"` in AOT release profile | -17% deploy binary | M | Requires `catch_unwind` boundary audit (R2 risk) |
| Cranelift `all-arch` opt-out (luna-aot) | -1 MiB CLI binary, -70 MiB build tree | M | Breaking change for users relying on cross-compile default |
| LZ4 bytecode compress (`.luna.bytecode`) | -40% on > 64 KiB payloads | M | Opt-in only; runtime decompression cost |
| Per-target precompiled staticlib ship | -30s cold AOT compile | L | Multi-target staticlib distribution |

All lever changes are out of scope for the v1.3 ship; they land in
the v2.0 Track DS implementation phase, with budget gating in the
`disk-baseline-2026-06-25` comparison pipeline.

## 5. Reproducing

Per-crate sizes:

```sh
for crate in luna-core luna-jit-derive luna-jit luna-runtime-helpers luna-aot; do
    cargo publish --dry-run -p $crate 2>&1 | grep -E "Packaged|Verifying"
done
```

AOT output binary:

```sh
echo 'print("hello")' > /tmp/hello.lua
cargo run --release -p luna-aot -- compile /tmp/hello.lua --out /tmp/hello
ls -lh /tmp/hello
strip /tmp/hello && ls -lh /tmp/hello
size -m /tmp/hello   # macOS Mach-O section breakdown
```

Full reproduction recipe: [`contributing-disk.md`](contributing-disk.md).

## 6. See also

- [`aot.md`](aot.md) §4 — AOT binary section breakdown
- [`deploy.md`](deploy.md) §2 — packaging shape decisions
- [`contributing-disk.md`](contributing-disk.md) — local
  reproduction guide
- [`architecture.md`](architecture.md) — 5-crate workspace layout
- `.dev/rfcs/v2.0-plan-state.md` §Track DS — full audit + budget
  feasibility tagging

---

*Last refreshed 2026-06-25 for the v1.3.0 ship + v2.0 Track DS
measurement baseline. The v1.1-era `cargo bloat` snapshot tables
have been retired — their `45.1% cranelift / 25.5% luna_core /
13.3% std` proportions are still directionally correct but the
absolute numbers reflected a different crate layout (pre-Stage-7
AOT, pre-derive crate). Use the per-target measurement scripts in
§5 to refresh for any commit.*
