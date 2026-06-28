# luna-tools

Developer-facing inspection + introspection CLIs for the luna runtime.

## Install

```sh
cargo install luna-tools
```

After install the following binaries are on `$PATH`:

| Binary | Status (v2.0 Track TL) | Purpose |
| --- | --- | --- |
| `luna-bin-inspect` | shipped (Phase 1) | Walk an AOT-produced binary's `.luna.bytecode` / `luna_trace_meta` / `luna_inline_chnx` sections and report a section table + trace index counts. |
| `luna-heap-dump` | shipped (Phase 1) | Run a `.lua` script in a `luna_jit::Vm`, then print a per-type heap snapshot (object count + approximate bytes). |
| `luna-trace-inspect` | shipped (Phase 2) | Run a `.lua` script and dump the resulting `JitStateSnapshot` — counters (compiled / closed / aborted / dispatched / deopt) plus active-trace head_pc + ops_len. `--show ir` / `--show mcode` are reserved for Track R IR overhaul + capstone wrapper respectively. |
| `luna-profile` | shipped (Phase 2) | Sampling profiler driven by a Count debug hook; text top-N or folded-stack output for `inferno-flamegraph`. `--format pprof` is reserved for the `--features flame-graph` opt-in (pprof + prost). |
| `luna-repl-polish` | Phase 2 stub | Polished REPL (multi-line, completion, smarter `~/.luna_history`). Pins `rustyline =14.x` once shipped to dodge the 14→15 API break. |

The remaining stub (`luna-repl-polish`) exits non-zero on real input with a pointer to `.dev/rfcs/v2.0-plan-state.md`, so the binary name is pinned even before the impl lands.

## `luna-bin-inspect` example

```sh
$ luna-bin-inspect ./my_aot_binary
luna-bin-inspect: ./my_aot_binary
  format: macho
  arch:   aarch64
  bytecode: 12345 bytes
  AOT trace index entries:   3
  AOT inline-chain entries:  7
  luna sections (5):
    NAME                                  SIZE  ADDR
    .luna.bytecode                       12345  0x100002000
    __DATA,luna_trace_meta                 144  0x100005010
    __DATA,luna_trace_blob                4096  0x1000050a0
    __DATA,luna_inline_chnx                168  0x1000060a0
    __DATA,luna_strkey_idx                  32  0x100006148
```

`--out json` emits the shared `luna_tools::schema::BinInspect` schema.

## `luna-heap-dump` example

```sh
$ luna-heap-dump my_script.lua
luna-heap-dump (luna 1.3.0)
  total: 445 objects, 34432 bytes (approx, shells only)
  buckets:
    TYPE                  COUNT   BYTES_APPROX
    str                     281          11240
    native                  137           6576
    table                    20           2240
    ...
```

`--out json` emits the shared `luna_tools::schema::HeapSnapshot` schema, designed to be the input format for a future `luna-heap-diff` tool.

## `luna-trace-inspect` example

```sh
$ luna-trace-inspect ./my_script.lua
luna-trace-inspect (luna 1.3.0)
  jit enabled       : true
  trace enabled     : true
  counters:
    compiled    : 1
    closed      : 1
    aborted     : 0
    dispatched  : 0
    deopt       : 0
  active trace      : <none>
```

`--format json` dumps the schema-versioned snapshot for downstream tooling. `--show ir` / `--show mcode` are reserved CLI surface; today they exit non-zero with a pointer to the Track R / capstone-feature tracking docs so user muscle memory is pinned even before IR shape stabilises.

## `luna-profile` example

```sh
$ luna-profile ./my_script.lua --every 100
luna-profile (luna 1.3.0)
  samples: 17 (every=100 insts), distinct stacks: 2
  top 2:
       COUNT    PCT  STACK (leaf → root)
          12  70.6%  ./my_script.lua:8 / ./my_script.lua:3
           5  29.4%  ./my_script.lua:8
```

`--format folded` emits one line per stack in the `frame_a;frame_b N` shape that `inferno-flamegraph` consumes on stdin:

```sh
$ luna-profile ./my_script.lua --every 100 --format folded | inferno-flamegraph > out.svg
```

`--format pprof` is reserved for the `flame-graph` feature opt-in (pulls `pprof` + `prost`).

## Workspace placement

`luna-tools` is a workspace member of `goliajp/luna`. It depends on `luna-jit` (which depends on `luna-core`). `luna-core` itself adds no third-party deps via this crate; the CI 0-dep gate continues to pass.

The pure-read accessors used by `luna-heap-dump` and (eventually) `luna-trace-inspect` / `luna-profile` live in `luna_core::vm::inspect` (re-exported as `luna_jit::inspect`). They are `&Vm`-only and allocate at most one small `Vec` per call — safe to drive from a hook callback or between dispatch ticks.

## Scope-split with Track AO

`luna-bin-inspect` currently ships as a stand-alone binary. Per `.dev/rfcs/v2.0-audit-tl.md`, Track AO can later expose the same logic as a `luna-aot inspect` sub-command; both surfaces would share the `luna_tools::schema::BinInspect` formatter.
