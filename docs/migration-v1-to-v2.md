# Migration: luna v1.x → v2.0

luna v2.0 is the first major-version break since v1.0. This page is
the consolidated migration guide for embedders moving from any v1.x
release. Each section below summarizes what changed and, where
applicable, gives the recipe to update call sites.

> **Status:** v2.0 is in-flight. Sections marked **TBD post-ship**
> are scaffolds that will be filled in at v2.0 release time once
> the corresponding tracks land. The shape and intent are stable;
> the exact code-level details are pending.

---

## Overview

### Stability contract carried forward from v1.x

luna v1.0 → v1.3 held a strict semver contract:

- Public `cargo doc` API: no breaking changes within v1.
- Embedder-visible sandbox defaults: no tightening that would break
  a working sandbox setup.
- Compiled luna-dialect bytecode: v1.x can load any earlier v1.x
  bytecode (forward-compatible).

v2.0 is allowed to break each of these surfaces. Everything that
*does* break is listed below.

### What does NOT change in v2.0

Even at a major bump, the following invariants are preserved:

- **`luna-core` 0 third-party deps.** Still ironclad; `cargo deny`
  gate intact.
- **0 `unsafe` at the embedder surface.** A4 still holds; the four
  `pub unsafe fn` remain `#[doc(hidden)]`.
- **`Vm: !Send + !Sync` baseline.** The `feature = "send"` track
  remains opt-in. Default `Vm` is still single-threaded.
- **Sandbox-by-default.** `Vm::sandbox(...)` still opens zero
  libraries and rejects bytecode loading by default.
- **PUC dialect coverage.** 5.1 / 5.2 / 5.3 / 5.4 / 5.5 all still
  supported.

---

## API renames

**TBD post-ship.** Track SQ (source-quality audit) flagged a
naming-inconsistency top-10 list (see
`.dev/rfcs/v2.0-audit-source-quality.md`). v2.0 collapses those
inconsistencies in one rename pass.

Each rename below will be expanded with:

- Old name → new name
- Crate / module / item kind
- `cargo fix`-style sed pattern if mechanical
- Rationale (which inconsistency it resolves)

```
TBD: rename list pending Track SQ landing.
```

Embedders can prepare by enabling the `#[deprecated]` warnings
luna v1.3 emits on the to-be-renamed surfaces; every deprecation
landing in v1.3.x will name its v2.0 replacement.

---

## Removed features

**TBD post-ship.** v2.0 retires a small set of v1.x APIs that were
either deprecated since v1.1 or shadow a cleaner replacement that
landed during the v2.0 cycle.

Expected categories (subject to track confirmation):

- v1.1-era `#[deprecated]` shims that were kept for one major.
- Internal `pub` items that leaked through the v1.0 surface and are
  now `pub(crate)`.
- Compatibility flags whose only consumer was a v1.x test.

The release notes for v2.0.0 will be the canonical removed-list;
this section mirrors that list with migration recipes.

---

## Deprecated → removed

**TBD post-ship.** The full deprecation ledger lives in CHANGELOG
entries for v1.1, v1.2, v1.3. Each entry tagged "scheduled removal
in v2.0" will get a row here with:

- File:line of the v1.x deprecation
- Replacement API
- Behavior difference, if any, beyond the name

---

## Compiler / bytecode-format breaking

**TBD post-ship.** Track PU (PUC polish) audited the dump-format
shape during v1.3 and flagged candidate cleanups for the next major.
Likely shape:

- luna's own dump format (`crates/luna-core/src/vm/dump/luna.rs`)
  may bump its magic-bytes minor revision; v1.x dumps may need a
  re-dump pass with the v2.0 compiler.
- PUC `.luac` translator coverage (Phase LB Wave 2 dialects) may
  extend to new opcodes; v1.x `.luac` files that loaded before will
  continue to load.
- The `cargo doc`-visible dump API may rename `dump_*` /
  `reader::*` items per Track SQ.

Concrete recipes — including how to detect the format version of an
existing dump and re-emit it — will land here at v2.0 ship.

---

## JIT API changes

**TBD post-ship.** Track J (JIT-aware SendVm interop) and the
side-trace ABI cleanup that ran across v1.2 → v1.3 left the
`JitState` / `IntChunkCompiler` / `TraceCompiler` traits in their
v1 shape. v2.0 may:

- Tighten the `IntChunkCompiler` / `TraceCompiler` trait surface so
  third-party backends compile cleanly against the post-cleanup
  shape.
- Promote `install_jit_backend` ergonomics (e.g. a builder taking
  the two compilers together) without removing the v1.x entry point.
- Re-shape `Vm::jit` (`JitState` sidecar from A2) public accessor
  if v1.x consumers exist (none known at v1.3 ship).

Embedders using only `Vm::install_jit_backend(chunk, trace)` and
`Vm::install_null_jit()` should not need changes; details on any
trait-method signature delta will land here.

---

## AOT binary breaking

**TBD post-ship.** `luna-aot` (introduced in v1.3 as Stages 1-7)
shipped its first stable binary format at v1.3.0. v2.0 may evolve:

- The object-file embed format (Stage 2 `bytecode-embed pipeline`)
  if the data-symbol layout changes (Stage 7 sub-piece 2 added
  string-key data symbols; further symbols may be added).
- The runtime-helpers staticlib symbol set (Stage 4) if new
  `luna_jit_*` helpers are needed by the lowerer.
- The `Vm::install_aot_trace` API surface introduced in Stage 7
  sub-piece 4.

AOT binaries produced by v1.3.x `luna-aot` will not be guaranteed
to load on v2.0 runtimes; recompile from source. The CLI subcommand
shape may also extend per Track TL.

---

## Performance changes

**TBD post-ship.** Track R (interpreter perf) and Track PI (PUC
parity polish) are expected to shift workload-by-workload numbers
in v2.0. Documented per-workload deltas will land here in the form
of a `vs v1.3.0` table:

```
TBD: per-workload table at v2.0 ship time. Reference the v1.3
baseline frozen in docs/performance.md.
```

Embedders should re-run their own perf gates against v2.0 and not
rely on v1.x absolute numbers.

---

## Tooling changes

**TBD post-ship.** `luna-aot` CLI is the primary tooling surface
that may grow new subcommands per Track TL. Other tooling areas:

- `cargo fmt` / `cargo clippy` / `cargo deny` profile pinning may
  bump MSRV; check `.github/workflows/msrv.yml` at ship.
- `cargo doc` output may reorganize per Track SQ rename pass.
- Workspace shape (currently 5 crates) is not expected to change in
  v2.0; if a sixth crate splits out, it will be listed here.

---

## Migration recipes

**TBD post-ship.** Once the rename / removal lists settle, this
section will host idiom-level before/after recipes for the most
common v1.x patterns:

- Sandbox setup
- Userdata trait migration (if any `LuaUserdata` API delta)
- Native function registration (if `native_typed` signature shifts)
- JIT backend installation
- AOT compilation pipeline

For each pattern, a short v1.x snippet and the equivalent v2.0
form, with a one-line explanation of why the shape changed.

---

## See also

- [`CHANGELOG.md`](../CHANGELOG.md) — full v1.x → v2.0 change log
  (canonical source for renamed / removed / added items)
- [`embedding.md`](embedding.md) — current (v2.0) cookbook
- [`security.md`](security.md) — sandbox boundaries (unchanged
  intent v1.x → v2.0)
- [`compatibility.md`](compatibility.md) — per-dialect feature
  matrix (PUC 5.1–5.5 coverage)
- [`architecture.md`](architecture.md) — crate layout (post-v1.3
  shape)
