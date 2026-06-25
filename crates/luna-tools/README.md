# luna-tools

Developer-facing inspection + introspection CLIs for the luna runtime.

## Install

```sh
cargo install luna-tools
```

After install the following binaries are on `$PATH`:

| Binary | Status (v2.0 Track TL Phase 1) | Purpose |
| --- | --- | --- |
| `luna-bin-inspect` | shipped | Walk an AOT-produced binary's `.luna.bytecode` / `luna_trace_meta` / `luna_inline_chnx` sections and report a section table + trace index counts. |
| `luna-heap-dump` | shipped | Run a `.lua` script in a `luna_jit::Vm`, then print a per-type heap snapshot (object count + approximate bytes). |
| `luna-profile` | Phase 2 stub | Sample-based flame-graph profiler (needs `--features flame-graph` once shipped — pulls `inferno` + `pprof`). |
| `luna-trace-inspect` | Phase 2 stub | Live JIT trace state dump. Gated on Track R IR shape stabilising — re-formatting now would break user muscle memory. |
| `luna-repl-polish` | Phase 2 stub | Polished REPL (multi-line, completion, smarter `~/.luna_history`). Pins `rustyline =14.x` once shipped to dodge the 14→15 API break. |

Stubs exit non-zero on real input with a pointer to `.dev/rfcs/v2.0-plan-state.md`, so the binary names are pinned even before the impls land.

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

## Workspace placement

`luna-tools` is a workspace member of `goliajp/luna`. It depends on `luna-jit` (which depends on `luna-core`). `luna-core` itself adds no third-party deps via this crate; the CI 0-dep gate continues to pass.

The pure-read accessors used by `luna-heap-dump` and (eventually) `luna-trace-inspect` / `luna-profile` live in `luna_core::vm::inspect` (re-exported as `luna_jit::inspect`). They are `&Vm`-only and allocate at most one small `Vec` per call — safe to drive from a hook callback or between dispatch ticks.

## Scope-split with Track AO

`luna-bin-inspect` currently ships as a stand-alone binary. Per `.dev/rfcs/v2.0-audit-tl.md`, Track AO can later expose the same logic as a `luna-aot inspect` sub-command; both surfaces would share the `luna_tools::schema::BinInspect` formatter.
