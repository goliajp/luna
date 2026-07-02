# Embedder recruitment call

**Status**: open — luna is looking for its second production
embedder to close v3.0 acceptance criterion #8.
**Since**: 2026-07-02 (v2.12.0)

---

## What luna offers

luna is a **Rust-native Lua VM** (Lua 5.1-5.5) intended for
long-lived embedder programs where the traditional
C-implementation trade-offs are costly:

- **Zero-third-party-dep contract on luna-core** — the
  interpreter crate depends on nothing outside `std`. Your
  supply chain shrinks.
- **`Send`-optional VMs** (feature-gated) — pin per-thread
  worker VMs, ferry roots between threads via
  `luna_jit::LuaRoot`.
- **Correctness-first** — passes PUC 5.1/5.2/5.3/5.4/5.5's
  official test suites (Linux + macOS) + differential
  parity vs PUC 5.5 across 100 luna-authored fixtures (as
  of v2.11.0).
- **Deterministic AOT** — `luna-aot embed` produces a
  standalone native binary that loads bytecode from a
  linker section, no runtime file I/O.
- **Cranelift-backed JIT** — trace-recording JIT for hot
  paths; interpreter fallback is always available.
- **Stable API contract** — `#![deny(missing_docs)]` +
  6-month SemVer clock (starting v2.7.0 = 2026-07-01).
  See [`embedding.md`](embedding.md) §13.

## What luna does NOT offer

Be upfront about limits, per
[`embedding.md`](embedding.md) §14:

- **NOT LuaJIT-parity perf**. luna's Cranelift-backed JIT
  is architecturally simpler than LuaJIT's SSA-IR
  type-specialized tracing JIT. See v2.9 structural ceiling
  analysis (`.dev/rfcs/v2.9-decomposition-and-ceiling.md`
  in the repo). Luna targets PUC interpreter parity, not
  LuaJIT speed.
- **Windows-specific gc.lua weak-table edge**. Rare pattern
  (`debug.sethook(..., "crl")` + `collectgarbage()` +
  weak-tables). Gated in CI, documented in embedding.md
  §14.
- **windows-11-arm target unsupported**. Awaiting upstream
  cranelift PE/COFF Aarch64 GOT relocation work.

## What we're looking for from you

- **Real production usage** — a system running luna as its
  Lua host, however small, for **≥1 month**. This closes
  v3.0 acceptance #8.
- **Bug reports** — file at
  https://github.com/goliajp/luna/issues with a repro. We
  prioritize embedder-hit bugs over speculative issues.
- **API friction feedback** — what feels awkward vs mlua,
  hlua, or Sol2/Sol3. Track DOCS-friendly gaps become PRs.

## How to start

1. Read [`embedding.md`](embedding.md) §1 (install) + §11
   (`Lua` front door). Most embedders only need `Lua`,
   `LuaFunction`, `LuaTable`, `LuaRoot`, `LuaSandboxBuilder`
   from the stable §13 contract.
2. Add `luna-jit = "2.12"` to your `Cargo.toml`. That's the
   front-door crate — pulls in luna-core, luna-jit-derive,
   luna-jit-helpers, luna-runtime-helpers transitively.
3. If you want AOT bytecode embedding into a standalone
   binary, `luna-aot = "2.12"` provides the tool chain.
4. Run through the [`architecture.md`](architecture.md) +
   [`threading.md`](threading.md) docs for topology
   patterns.

## First-embedder reference: `kevy`

The first production embedder is
[`kevy`](https://github.com/goliajp/kevy), a Rust
reimplementation of a
Redis-compatible KV store, using luna for `EVAL` /
`EVALSHA` script execution. kevy's dogfood report from
2026-06-23 seeded much of the v1.1 → v2.x API surface.

kevy uses luna's `send` feature-gated VM to pin one Vm per
worker, `LuaRoot` for pinning across yields, and the AOT
bytecode section for shipping precompiled scripts inside
the binary.

## Talking to the maintainers

- GitHub issues for bugs + API friction
- GitHub discussions for design questions ("should luna do
  X?", "how do you handle Y in an embedder?")
- crates.io release channel: `cargo add luna-jit` pulls the
  latest; version bumps follow the SemVer contract in
  §13 of embedding.md

We appreciate your evaluation. Even a "we tried luna and
switched back" report is useful signal.
