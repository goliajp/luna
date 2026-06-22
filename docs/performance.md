# Performance

Cross-dialect microbench snapshot at v1.0.0 (2026-06-23). Bench:
`cargo bench --bench cross_dialect` on M-series macOS, release +
LTO. `vs.X = luna_time / X_time` (lower = luna faster).

PUC and LuaJIT times include subprocess startup overhead
(~50-200 µs per cell); luna numbers are stable in-process.

---

## Master gate

luna's master gate is `vs.X ≤ 0.50` — luna at least 2× faster than
the reference — on every cell × dialect × reference pair:

| | cells | pass | fail |
|---|---:|---:|---:|
| vs PUC 5.1-5.5 (7 cells × 5 dialects) | 35 | **35** ✓ | 0 |
| vs LuaJIT 2.1 (7 cells) | 7 | 6 ✓ | 1 |
| **Total** | **42** | **41** | **1** |

The single sub-gate cell is `binary_trees_n10` vs LuaJIT 2.1 (0.83×).
luna is still 1.21× faster than LuaJIT on this workload — just not
2× faster.

## Per-dialect cell numbers

### vs PUC 5.1

| cell | luna (µs) | PUC (µs) | vs.puc |
|---|---:|---:|---:|
| fib_28 | 1616 | 19000 | 0.08 ✓ |
| loop_int_1m | 755 | 4779 | 0.16 ✓ |
| table_alloc_10k | 337 | 2694 | 0.13 ✓ |
| string_concat_5k | 846 | 3242 | 0.26 ✓ |
| math_loop_100k | 648 | 6697 | 0.10 ✓ |
| closure_alloc_10k | 972 | 2915 | 0.33 ✓ |
| binary_trees_n10 | 2714 | 6437 | 0.42 ✓ |

### vs PUC 5.2

| cell | luna (µs) | PUC (µs) | vs.puc |
|---|---:|---:|---:|
| fib_28 | 1619 | 16000 | 0.10 ✓ |
| loop_int_1m | 743 | 4982 | 0.15 ✓ |
| table_alloc_10k | 344 | 2911 | 0.12 ✓ |
| string_concat_5k | 903 | 3208 | 0.28 ✓ |
| math_loop_100k | 651 | 6459 | 0.10 ✓ |
| closure_alloc_10k | 932 | 2930 | 0.32 ✓ |
| binary_trees_n10 | 2746 | 5858 | 0.47 ✓ |

### vs PUC 5.3

| cell | luna (µs) | PUC (µs) | vs.puc |
|---|---:|---:|---:|
| fib_28 | 1087 | 16000 | 0.07 ✓ |
| loop_int_1m | 508 | 5294 | 0.10 ✓ |
| table_alloc_10k | 189 | 2865 | 0.07 ✓ |
| string_concat_5k | 463 | 2654 | 0.17 ✓ |
| math_loop_100k | 643 | 6062 | 0.11 ✓ |
| closure_alloc_10k | 986 | 2908 | 0.34 ✓ |
| binary_trees_n10 | 2371 | 5768 | 0.41 ✓ |

### vs PUC 5.4

| cell | luna (µs) | PUC (µs) | vs.puc |
|---|---:|---:|---:|
| fib_28 | 1091 | 12000 | 0.09 ✓ |
| loop_int_1m | 259 | 5474 | 0.05 ✓ |
| table_alloc_10k | 193 | 2992 | 0.06 ✓ |
| string_concat_5k | 466 | 2756 | 0.17 ✓ |
| math_loop_100k | 627 | 6313 | 0.10 ✓ |
| closure_alloc_10k | 1037 | 3110 | 0.33 ✓ |
| binary_trees_n10 | 2423 | 6208 | 0.39 ✓ |

### vs PUC 5.5

| cell | luna (µs) | PUC (µs) | vs.puc |
|---|---:|---:|---:|
| fib_28 | 1107 | 13000 | 0.08 ✓ |
| loop_int_1m | 260 | 5144 | 0.05 ✓ |
| table_alloc_10k | 190 | 2928 | 0.07 ✓ |
| string_concat_5k | 460 | 2641 | 0.17 ✓ |
| math_loop_100k | 596 | 6097 | 0.10 ✓ |
| closure_alloc_10k | 984 | 2957 | 0.33 ✓ |
| binary_trees_n10 | 2447 | 5894 | 0.42 ✓ |

### vs LuaJIT 2.1

| cell | luna5.1 (µs) | LuaJIT (µs) | vs.ljit |
|---|---:|---:|---:|
| fib_28 | 1607 | 3381 | 0.48 ✓ |
| loop_int_1m | 752 | 2495 | 0.30 ✓ |
| table_alloc_10k | 333 | 2101 | 0.16 ✓ |
| string_concat_5k | 862 | 2222 | 0.39 ✓ |
| math_loop_100k | 629 | 2145 | 0.29 ✓ |
| closure_alloc_10k | 951 | 2622 | 0.36 ✓ |
| binary_trees_n10 | 2679 | 3240 | **0.83 ❌** |

## Design ceiling note

`binary_trees_n10` is luna's hardest cell because of the table-
allocation density. luna's `Value` is a 16-byte tagged enum
(`#[repr(C, u8)]`) — chosen for hot-path arithmetic performance
(NaN-boxing the 8-byte alternative regresses arith 24-98% on
realistic VM loops). The Lua bytecode layout is also preserved for
PUC binary compatibility.

Closing the `binary_trees` gap to LuaJIT's 2× ratio would require
breaking either:

- The 16-byte Value layout (NaN-box → lose arith)
- PUC bytecode binary compat (introduce a different layout that
  packs frame metadata into stack memory)

Both are explicit project constraints. luna's 1.21× speedup over
LuaJIT 2.1 on this single workload is therefore the design ceiling,
not an optimization gap.

## Test environment

- macOS 25.5 / aarch64 (M-series, Apple Silicon)
- rustc 1.86+
- cranelift 0.124
- PUC binaries: Lua 5.1.5, 5.2.4, 5.3.6 (built from PUC source);
  Lua 5.4.8, 5.5.0 + LuaJIT 2.1.1781602682 (via brew)
- Bench: `cargo bench --bench cross_dialect` (median of N iters
  per cell)
