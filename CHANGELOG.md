# Changelog

All notable changes to luna will be documented in this file. Format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The public stability contract for the 1.x line covers:

- `pub` items in `src/lib.rs`'s exported tree
  (`luna::vm::Vm`, `luna::runtime::Value`, `luna::version::LuaVersion`,
  `luna::frontend::*` parser surface)
- The `lua.h`-compatible C ABI under `src/capi.rs`
- Bytecode binary compatibility with PUC Lua per-dialect (`.luac`
  files load in and out)

Internal modules (JIT codegen, dispatcher hot-path internals, heap
internals) may change without notice within 1.x for performance
optimization.

---

## [3.0.0] — 2026-08-14

The v2.x maturity arc's destination. **No breaking change** — the major
bump marks the maturity gate, not an API break. `luna_core`'s public
surface is identical to 2.18.0 (788 items, zero removals, zero
additions), and code building against 2.x builds against 3.0 unchanged.

### What 3.0 asserts

The arc opened (2026-06-28) with a list of ten things that had to be
true before luna could be called mature. Where each landed:

| | Criterion | How it is evidenced today |
|---|---|---|
| 1 | No known UAF / heap corruption | ASAN nightly over the full PUC official suite across 5 dialects, 48 consecutive greens; Miri on the library plus a regression subset; zero open known-bugs |
| 2 | Differential parity with PUC 5.1–5.5 | 514 private-corpus fixtures byte-identical to stock PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.8 / 5.5.1, zero skips, gated on every push; the official suite passes end-to-end on all five dialects |
| 3 | 24h-equivalent soak clean | Four capped runs, ~23h cumulative, second-half RSS drift under 1% with no vm_mem accumulation; six later samples corroborate |
| 4 | Cross-allocator clean | glibc + jemalloc + mimalloc + Apple-malloc, nightly |
| 5 | Cross-platform matrix green per push | ubuntu / macos / windows / ubuntu-arm × stable, plus wasm32 targets, perf-gate, feature matrix and the zero-dependency contract |
| 6 | Perf floor closed or ceiling explicit | v2.9 established a structural ceiling with a decomposition record |
| 7 | API stability | Enforced per release by a public-surface audit: a minor bump must show an empty removal list |
| 8 | *(removed from the gate)* | Adoption is a product outcome, not an engineering one; tracked as an observed fact rather than a precondition |
| 9 | Documentation complete | Every public item documented, `deny(missing_docs)` |
| 10 | Fuzz corpus established | 7 targets on a weekly schedule, with a gate that fails the run if any target produces a crash artifact |

### Changed since 2.18.0

Nothing in the runtime. This release is the arc's closing record plus CI
and tooling repair:

- **`luna-aot` cross-compilation errors now give correct advice.** The
  "CARGO_MANIFEST_DIR not set" message suggested setting
  `LUNA_AOT_RUNTIME_HELPERS_STATICLIB`, which is honoured for host
  builds only — following that hint on a cross target produced the same
  error with nothing left to try. Cross builds are now told what
  actually works (run luna-aot from inside its workspace) and why the
  override does not apply. **The underlying limitation stands and is
  worth knowing: a standalone `luna-aot` binary cannot cross-compile**,
  because a cross target always builds its own runtime-helpers staticlib
  and that needs the workspace.
- Two CI jobs that had never once completed — Alpine musl E2E and Wine
  PE execution — were repaired and are now blocking. Between them they
  had been masking a real product path (standalone cross-compilation)
  and four CI defects.
- Toolchain-lint drift now gets previewed on beta and nightly ahead of
  each stable release; the LuaJIT differential reference is asserted
  rather than inherited from the runner image; the site's links are
  link-checked; dependency major-version lag is measurable.

### For embedders

- **No migration needed.** Public API unchanged from 2.18.0.
- luna declares **no MSRV** (since 2.17.0). The promise is "builds on
  current stable", which the four-platform matrix tests on every push.
  Informationally, the tree currently needs rustc 1.88+.
- The dialect-sensitive behaviour corrected in 2.18.0 (5.1/5.2 type-error
  wording, table-library surface and raw element access, `__len` on 5.1)
  is unchanged here — see that entry if you are coming from 2.17 or
  earlier.

## [2.18.0] — 2026-08-14

Upstream reference moved to **PUC Lua 5.5.1** (released 2026-07-24), and
six real defects were found and fixed in the process. One is a denial of
service; three change behaviour on the 5.1/5.2 dialects.

### Fixed
- **`string.rep` hung the VM when the piece was empty (DoS).**
  `string.rep("", math.maxinteger, "")` looped `n` times copying zero
  bytes; the size guard cannot catch it because `0 * n` never overflows.
  A single expression could hang any embedder running untrusted Lua. luna
  matched PUC 5.5.0 here, which has the same bug; PUC fixed it in 5.5.1.
- **The table library's surface ignored the dialect.** luna registered
  the union of every version's functions, so 5.1 saw
  `table.create`/`move`/`pack`/`unpack` and 5.2 saw `create`/`move`,
  while 5.1 was missing `setn` and 5.2 was missing `maxn`. Code written
  against an older dialect ran here and broke on real PUC. Registration
  is now ordered by version, and `table.setn` exists on 5.1 to raise
  "'setn' is obsolete" exactly as PUC does.
- **Type errors were worded wrongly on 5.1/5.2.** PUC ≤5.2 names the
  operand first — `attempt to call field 'f' (a nil value)`; 5.3 flipped
  to type-first. luna emitted the 5.3+ form everywhere, so every type
  error under 5.1/5.2 (calls, indexes, arithmetic) had the wrong shape.
  ≤5.2 also has no metamethod operand names, so those collapse to the
  bare message.
- **Table-argument checking and element access now follow the dialect.**
  PUC changed this twice: 5.1/5.2 demand a real table and read elements
  raw (`lua_rawgeti`); 5.3+ accept anything with the needed metamethods
  (`checktab`) and read through `__index` (`lua_geti`); 5.5 additionally
  exempts strings from the `__len` requirement and is the first version
  to type-check `table.unpack` at all. luna combined a hard check with
  metamethod access, matching no dialect. Visible effect, concatenating a
  proxy whose contents sit behind `__index`: 5.1 gives `""`, 5.2 raises
  `invalid value (nil) at index 1`, 5.3+ reads through.
- **`__len` no longer applies to tables on 5.1.** PUC 5.1's `__len` is a
  userdata-only metamethod, so `#setmetatable({}, {__len = f})` is `0`
  there and `7` on 5.2+. luna called the metamethod on every dialect.
  This governs every `#` on 5.1, not just the table library.
- **Errors naming a value read from a named-vararg table lost the field
  name.** `t.k` inside `function f(...t)` compiles to a dedicated opcode
  rather than `GETFIELD`, and the operand-naming walk did not recognise
  it: `attempt to perform arithmetic on a nil value` instead of
  `… (field 'xx')`.
- **Locals left scope before their block's `CLOSE` ran.** A `__close`
  handler calling `debug.getlocal` on the enclosing frame saw
  `(temporary)` rather than the variable name, because the block's
  `OP_CLOSE` was emitted after each local's scope had been closed off.
  Most visible in a `repeat` body, where the exit path was affected and
  the loop-back path was not.

### Changed
- Differential basis is now PUC **5.5.1** throughout: the corpus builds
  against the 5.5.1 tarball, `tests/official/` carries the 5.5.1 suite,
  the fuzz workflow's reference interpreter is 5.5.1, and CI asserts the
  5.5 interpreter is exactly 5.5.1 rather than accepting whatever the
  runner image ships.
- Private differential corpus 500 → **514** fixtures, byte-equal against
  stock PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.8 / 5.5.1 with zero skips.

### Note on how these were found
Re-running the corpus against 5.5.1 produced **zero** divergences. That
was not reassurance — it meant the corpus could not reach what upstream
had changed. Reading the 5.5.0→5.5.1 source diff and probing the
behaviour it touched, three ways (5.5.0 / 5.5.1 / luna), is what surfaced
all six defects. A 210-cell dialect matrix (7 argument shapes × 6 table
operations × 5 dialects) went from 27 divergent cells to 0.

## [2.17.0] — 2026-08-14

Maintenance release. No runtime behaviour changes; the VM, its dialect
semantics, and the C ABI are untouched. Everything here is metadata,
tooling, or disclosure — but two items materially affect embedders, so
read Changed before upgrading.

### Removed
- **luna no longer declares an MSRV.** `rust-version` is gone from the
  workspace and from all nine crate manifests, and the `msrv` CI
  workflow is deleted.

  It had said `1.86` since v1.1.0 and that was **false the entire time**:
  the tree has used edition-2024 let-chains since `2bbda24`
  (2026-06-23), and let-chains need rustc **1.88**. Measured: 1.87 fails
  with `E0658: 'let' expressions in this position are unstable`. Eleven
  releases shipped a compatibility promise the code did not keep.

  Nobody caught it because the `msrv` workflow triggered on a `main`
  branch this repository does not have — it never executed once. An
  unverified pin is worse than no pin: it misleads people who trust it.

  Rather than re-pin and carry the upkeep, luna makes no MSRV promise.
  What it is willing to guarantee is "builds on current stable", which
  is what the CI matrix actually tests on every push across ubuntu /
  macos / windows / ubuntu-arm.

  **Informational, not a contract** (may move without a version bump):
  as of 2.17.0 the tree needs **rustc 1.88+**. With no declared floor,
  an older toolchain surfaces as a compiler error (`E0658`) rather than
  a cargo manifest error — if you see that, upgrade rustc rather than
  suspecting your own code.

### Fixed
- **Disclosure — `numeric::num_to_string_for` changed signature in
  v2.14.0 and this was not announced.** The parameter went from
  `legacy_float: bool` to `fmt: FloatFmt`:

  ```rust
  // <= 2.13.0
  pub fn num_to_string_for(n: Num, legacy_float: bool) -> String
  // >= 2.14.0
  pub fn num_to_string_for(n: Num, fmt: FloatFmt) -> String
  ```

  `luna_core::numeric` is a `pub mod`, so this is inside the stability
  contract stated at the top of this file, and it shipped in a *minor*
  release — a semver violation on our part. The v2.14.0 entry mentioned
  the new `numeric::FloatFmt` type under **Fixed** but never said the
  function's signature had changed, and carried no `BREAKING` marker.

  No compatibility shim is being added: a `bool` cannot express the three
  per-dialect float formats v2.14 had to distinguish, so the enum is the
  correct shape and restoring the old overload would permanently carry an
  API that cannot say what the function does.

  Migration — pass the variant instead of the bool:

  | Old call | New call | Dialects |
  |---|---|---|
  | `num_to_string_for(n, true)` | `num_to_string_for(n, FloatFmt::Legacy14)` | 5.1, 5.2 (`%.14g`, no `.0`) |
  | `num_to_string_for(n, false)` | `num_to_string_for(n, FloatFmt::TwoStage55)` | 5.5 (`%.15g` → round-trip → `%.17g`) |
  | *(not expressible)* | `num_to_string_for(n, FloatFmt::G14)` | 5.3, 5.4 (`%.14g` + `.0`) |

  The third row is the reason for the change: v2.13 applied the 5.5
  scheme to 5.3/5.4 as well, which was one of the divergences v2.14
  fixed. `FloatFmt` has no constructor from a `LuaVersion` — pick the
  variant directly, as the VM itself does.

  Found by the public-surface audit now run every release
  (786 → 788 public items over v2.13.0→HEAD; this was the only
  removal/signature change).

### Internal
- Cleared 25 `clippy` 0.1.97 findings (22 `collapsible_if` → edition-2024
  let-chains, plus `unnecessary_parens`, a redundant `&`, and a
  `match` → `?`). CI had been red for 34 days. The trigger was an
  unreleased commit raising the MSRV declaration to `1.97`: clippy's
  let-chain suggestion is MSRV-gated, so a higher declared floor
  unlocked it. With no MSRV declared at all, clippy assumes current
  stable and the tree is clean — verified.
- `cargo-deny` now runs with `--all-features`. It had been resolving only
  the default feature set, leaving every crate behind luna-tools' opt-in
  `flame-graph`/`mcode-disasm` features unscanned; restoring coverage
  surfaced a yanked `spin 0.10.0` (bumped to 0.10.1).
- `soak-weekly` now finishes inside the runner cap instead of being
  killed every week — 11 consecutive `cancelled` runs made the job
  useless as a signal even though its artifacts were valid. Six
  backlogged samples analysed: 6/6 clean, second-half RSS drift
  0.414%–0.899%, all under the 1% bar.
- `actions/checkout` and `actions/upload-artifact` upgraded v4 → v7.

## [2.16.0] — 2026-07-06

### Changed
- **v3.0 differential-parity acceptance narrowed to two legs.** The
  official-suite *byte-diff* leg is dropped from the acceptance set;
  parity is now proven by (a) the 500-fixture private corpus being
  byte-identical to PUC across all five dialects, gated on every push,
  and (b) the official suite's assert-count instrumentation running
  nightly. Measured divergence on the byte-diff harness was 24% (against
  a 5% estimate), split between files where the harness's body-wrap
  breaks PUC's own scope semantics and genuine numeric-formatting
  internals; closing it would have needed a per-dialect allowlist roughly
  twice the size the design permitted.

### Added
- Opt-in official-suite byte-diff harness (`LUNA_OFFICIAL_BYTE_DIFF=1`,
  with `PUC_LUA_5X` binaries) as a local diagnostic surface for
  investigating per-file divergence. Not a CI gate.

## [2.15.0] — 2026-07-06

### Added
- **Differential corpus 400 → 500 fixtures**, every one byte-equal to
  its stock PUC interpreter with zero skips: 15 compiler-shape and 25
  utf8 fixtures for 5.5, a 14-fixture 5.4 batch (to-be-closed variables,
  `<const>`, `coroutine.close`, generational GC, `__pairs`), and 43
  fixtures across 5.1/5.2/5.3. Every dialect now carries at least 25
  fixtures (5.1=25, 5.2=25, 5.3=25, 5.4=25, 5.5=400).
- ASAN nightly now runs the full official suite across all five dialects,
  measured at 5m34s under Docker and 2–3 min on hosted runners.

### Changed
- **Miri's acceptance leg narrowed to the library plus a representative
  integration subset.** Full `official_run` under Miri measured at 30 min
  – 2.5 h per nightly; the marginal provenance/UB coverage beyond the
  `--lib` gate is small against luna's bounded unsafe surface (~2000 LOC),
  and the combinatorial surface it would add is what the ASAN gate above
  already covers.

## [2.14.0] — 2026-07-05

### Added
- **Multi-dialect differential harness** — the luna-vs-PUC diff
  corpus now runs per dialect: `tests/diff_puc/5.1/ … 5.5/`
  subtrees execute against stock PUC 5.1.5 / 5.2.4 / 5.3.6 /
  5.4.8 / 5.5.0 interpreters (built from source in CI). Ground
  truth is each version's DEFAULT `make` build, compat flags
  included.
- **Error-channel comparison** — `*_err.lua` fixtures pin the
  error path: both interpreters must fail at top level (non-zero
  exit ⇔ eval Err) with matching normalized error text.
- **Corpus 250 → 400** — dialect seed batches (10+ per legacy
  dialect), io/os deterministic batch, 33-fixture error-channel
  batch, coroutine deep matrix, string.pack format matrix,
  metamethod/core-semantics batch.
- `Vm::error_display` — renders an error value the way PUC's
  standalone message handler does: numbers stringify and
  non-string objects get their `__tostring` called before
  collapsing to the `(error object is a … value)` tag. The
  non-executing `Vm::error_text` is unchanged.

### Fixed
23 real divergences against stock PUC interpreters, including:
- **Per-dialect float formatting** — ≤5.2 prints `%.14g` with no
  `.0` suffix, 5.3/5.4 `%.14g` + `.0`, 5.5 the two-stage
  `%.15g`→`%.17g` (new `numeric::FloatFmt`; v2.13 had applied the
  5.5 scheme to every dialect).
- **Per-dialect arithmetic error wording** — 5.4+ report
  string-involved arithmetic faults as `attempt to add a 'string'
  with a 'number'` (lstrlib's string-metatable handlers); ≤5.3
  keep the aggregate wording.
- **`error(nil)` substitution timing** — `<no error object>` is
  substituted only at the catch point (after a message handler
  ran), so xpcall handlers and the top level see the raw nil,
  matching `luaG_errormsg`.
- Dialect-gated stdlib surface: `loadstring` exists on 5.2 and
  `bit32` on 5.3 (stock `LUA_COMPAT_*` builds); `math.type` /
  `tointeger` / `ult` are 5.3+; 5.1 `xpcall` does not forward
  extra arguments; 5.4/5.5 boot in generational GC mode.
- `os.date` implements the C99 strftime specs PUC inherits
  (`%u %e %C %D %T %F`) and the ISO 8601 week date (`%G`/`%V`).
- luaL-conformant wording/returns: `file:setvbuf`/`file:flush`
  return `true`; `string.rep`/`gsub`/`format` integer arguments
  follow `luaL_checkinteger` (numeric strings convert, failures
  say `bad argument #N … (number expected, got T)`);
  pcall/xpcall-invoked natives qualify their names via
  `package.loaded` (`'coroutine.resume'`, not `'resume'`);
  `table.concat` element errors carry the type name;
  `invalid key to 'next'` has no position prefix.

### Changed
- **CI perf-gate is now a same-runner comparison** — the fixed-ns
  baseline was invalid on heterogeneous hosted runners (identical
  code measured 0.505x–1.087x across runs). The gate benches a
  pinned reference commit on the same runner immediately before
  HEAD and diffs the two. Same-runner verdict for this release:
  every cell within 0.98x–1.03x of v2.13.0.

## [2.13.0] — 2026-07-04

### Fixed
- **Stacked Borrows UB in Table/LuaClosure inline storage** — the
  cached self-referential pointers (`array_ptr` / `upvals_ptr`,
  from the P11-S5d inline-array and closure-inline optimizations)
  were invalidated by every `&mut self` function-entry retag;
  accesses through them were undefined behavior with real
  miscompilation risk under rustc's noalias annotations. Inline
  storage now lives in `UnsafeCell` and accessors derive the base
  pointer fresh at each use; the Miri nightly lane passes for the
  first time since it landed.
- **UAF-C closed** — the Windows gc.lua `STATUS_ACCESS_VIOLATION`
  (gated since v2.4, perma-gated v2.8 as "repro infeasible") was
  root-caused to two platform-independent GC bugs and fixed:
  an explicit `collectgarbage()` collected with a stale stack-root
  cursor and swept its caller's live register values (fix: PUC
  C-call discipline — entering a native raises the cursor to the
  argument top), and weak-table tombstone keys (`t[k] = nil`)
  escaped the clear-key sweep and were freed while hash-chain walks
  still compared them (fix: PUC `clearbykeys`-style unconditional
  key demotion on empty entries). The Windows CI gate on
  gc.lua/gengc.lua/tracegc.lua is physically removed; validated by
  25× Linux ASAN stress and 6 consecutive 50-iteration Windows
  stress runs on native-heap and poison-allocator lanes.
- `coroutine.resume` status errors ("cannot resume dead coroutine"
  etc.) no longer carry a position prefix, matching PUC
  `resume_error`.
- `debug.getupvalue` / `debug.setupvalue` return zero values (not
  nil) for out-of-range indices, matching PUC `db_getupvalue`.
- Float `tostring` now matches PUC 5.5 byte-for-byte: two-stage
  `%.15g` → round-trip check → `%.17g` (lobject.c
  `tostringbuffFloat`), replacing Rust's shortest-round-trip
  spelling (`math.pi` now prints `3.1415926535897931`).
- `error()` invoked directly by a C function (e.g.
  `pcall(pcall, error, "msg")`) no longer carries a position
  prefix — `luaL_where` now counts continuation activations as C
  frames.

### Added
- `gc-verify` feature (zero-dep, diagnostic): luna's
  `lua_checkmemory` analogue — post-sweep dangling-reference walk,
  atomic-phase tricolor invariant check, post-collect rooted-stack
  liveness audit, and freed-pointer read-time probes.
- Differential corpus 150 → **250** fixtures, all byte-equal
  against PUC 5.5 with zero skips (metamethod full sweep, coroutine
  edges, string.format matrix, pattern engine sweep, float-spelling
  matrix, and more). Pinned 5.5 semantics: `__pairs` restored,
  generic-for control variable is `<const>`.
- `uafc-windows-stress.yml` dispatch workflow: procdump-hosted
  gc.lua stress on windows-latest with cdb stack capture.
- luna-soak writes its JSON report incrementally after every
  sample, so a run killed by the GHA 6-hour job cap still uploads
  a complete partial report.

---

## [2.12.0] — 2026-07-02

- Differential corpus 100 → 150 fixtures, all byte-equal PUC 5.5
  with **zero skips** — the harness now fails loudly when a fixture
  errors on PUC itself (previously silently skipped; 5 never-diffed
  broken fixtures repaired).
- `math.modf` returns its integral part as an integer (PUC
  5.4/5.5 semantics); arithmetic-on-string error wording documented
  as a deliberate cross-dialect design (PUC 5.4 wording retained).
- soak pipeline zero-data root cause fixed (see Unreleased:
  incremental report write) and 4 latent CI bugs repaired — the
  diff-puc workflow went green for the first time since landing.
- `docs/embedder-recruitment.md` — public call for luna's second
  production embedder.

## [2.11.0] — 2026-07-02

- Differential corpus 45 → 100 fixtures.
- `#![deny(missing_docs)]` across the public API (was warn).

## [2.10.0] — 2026-07-02

- Differential corpus 5 → 45 fixtures across arithmetic, strings,
  tables, closures, coroutines, metamethods, and stdlib edges.
- Docstring audit closed (v3.0 acceptance #9).

## [2.9.0] — 2026-07-01

- Perf positioning finalized: the LuaJIT 1.18× charter floor is
  documented as a structural ceiling (trace-JIT architecture
  class); luna's lane is the correctness-first Rust-native Lua VM
  competitive with the PUC 5.4/5.5 interpreter. Full decomposition
  evidence in-tree.

## [2.8.0] — 2026-07-01

- Public API stability contract documented
  (`docs/embedding.md` §13); the 6-month API-stability clock for
  v3.0 acceptance #7 starts at v2.7.0.
- Windows gc.lua UAF gated + documented as a known limitation
  (superseded in v2.13 — root-caused and fixed).
- Windows arm64 AOT deferred (vendored cranelift PE/COFF Aarch64
  GOT relocations unimplemented upstream).

## [2.7.0] — 2026-07-01

- Per-PR perf-gate made required (redis_lua_shape 5-cell bench vs
  committed baseline, 1.05× regression threshold).
- Embedder API audit; cross-platform CI matrix extended with
  Linux arm64 (ubuntu-24.04-arm).

## [2.6.0] — 2026-06-30

- Per-PR perf-gate infrastructure (advisory) + nightly
  luna-vs-LuaJIT differential lane + Boltzmann program-generator
  grammar extension.
- Frame-pop slot-clear chain completed (P1B-2E minimal
  tightening) — later found to be one origin of UAF-C (see
  Unreleased).

## [2.5.0] — 2026-06-30

- Slot-clear discipline at every Lua-side frame-pop site
  (`finish_results`, `Op::TailCall` collapse, pcall unwind),
  mirroring PUC's `L->top` hygiene.

## [2.4.0] — 2026-06-29

- Soak-test harness (`luna-soak`: RSS + `Vm::memory_used`
  sampling with JSON reports) + nightly 1h / weekly 24h-capped
  soak workflows.
- cargo-fuzz corpus seeding + nightly fuzz CI hardening.

## [2.3.0] — 2026-06-29

- UAF-A fully closed (sort.lua AA load+collectgarbage SIGSEGV) —
  `finish_results` slot-clear root fix; every v2.2.0 CI byte-strip
  and env-skip gate physically removed.
- ASAN and Miri nightly CI lanes.

## [2.2.0] — 2026-06-28

- UAF-B closed (toomanyidx memory-cap SIGSEGV under glibc).
- Test-infrastructure foundation per the v2.x → v3.0 maturity arc
  charter: ASAN docker environment, differential-vs-PUC harness
  (first 5 fixtures), cross-allocator test matrix groundwork.

---

## [2.1.0] — 2026-06-28 (the v2.0 mega sprint)

> Shipped 2026-06-28 to crates.io as 7 crates (luna-core /
> luna-jit-derive / luna-jit-helpers / luna-jit-llvm / luna-jit /
> luna-runtime-helpers / luna-aot), 235 commits since v1.3.0.
> The sprint log below is preserved as shipped-scope record —
> 14 tracks (J/R/PI/AO/MM/DS/CV/DO/PU/AT/TL/BM/CB/SQ) per
> the v2.0 charter, collapsed from what would have been
> v1.4–v1.8 under the `nodefer` directive.

### Phase 0 — 13 parallel audits (2026-06-25)

- Tracks J / R / PI / AO / MM / DS / CV / DO / PU / AT / TL / BM /
  CB each spawned a read-only audit agent. 13 RFCs landed; PI's
  full 26 KB body preserved;
  12 others' summaries (100–150 word + top-3 risks each) inlined as
  truth-of-record in the plan state.
- v2.0 Track SQ (textbook-grade source quality) added as Track 14
  per user request mid-Phase-0, sequenced LAST. Audit at
  the source-quality audit (45 KB / 821 lines).

### Phase 1 — Correctness backfill (CB)

- **CB-pre1** + **CB-pre2** verify-and-archive: pre-existing v1.0
  debug-mode SIGTRAP + `debug_upvalue_order_and_id` flakiness both
  cleared by the v1.3 fix chain (`fae0f9c` / `e5db587` / `f8afd64`);
  bug docs moved to the fixed pile.
- **CB-or** assert-counter wrapper at
  `crates/luna-core/tests/official_run.rs` + per-PUC-file coverage
  report. 140 PUC files
  exercised, 2.4M asserts reached, 2.36M passing, 101/140 files at
  ≥80% hit rate, 27/29 below-80% are PUC-internal early-return
  shims, 2 wrapper carve-outs (`errors.lua` / `db.lua` × 5 dialects).
- **CB-edge** 13 spot tests pinned: 5 GC finalizer
  (`cb_edge_gc_finalizer.rs` — recursive collect / weak-key+finalizer
  / userdata-as-key / 1000-proxy stress / error-in-`__gc`) + 3
  coroutine + hook (`cb_edge_coroutine_hook.rs`) + 6 compiler
  stress (`cb_edge_compiler_stress.rs` — 2000-stmt fn body /
  150-deep + 250-deep nesting cap / 60-deep paren / 100 upvals /
  50-arg vararg forward).
- **CB-edge real bug surfaced + fixed**: `Vm::set_hook` predicate
  `target.is_none()` arm was missing — `debug.sethook(…)` called
  from inside a coroutine body silently dropped. Root cause at
  `crates/luna-core/src/vm/exec.rs:2151-2179`; regression test +
  sibling test landed; the known-bug doc moved to the fixed pile.

### Phase 2 — Coverage + fuzz infrastructure (CV-infra)

- New workspace-excluded `crates/luna-fuzz/` crate with 4
  `fuzz_target!` harnesses: parser, dump_reader, vm_dispatch,
  aot_meta. Nightly toolchain pinned via crate-scoped
  `rust-toolchain.toml`.
- `.github/workflows/coverage.yml` — `cargo llvm-cov` workspace
  vs committed JSON baseline; fails PR on > 2pp regression in any
  first-party crate.
- `.github/workflows/fuzz.yml` — 5-min PR smoke (non-blocking) +
  60-min weekly cron per target.
- luna-core 0-third-party-dep contract intact: `libfuzzer-sys` lives
  only in the excluded fuzz crate.

### Phase 3 — Docs CI gate (DO-CI)

- `.github/workflows/docs.yml` — `cargo doc -D warnings` +
  `cargo test --doc` + `lychee` link check.
- 1 pre-existing intra-doc warning in `crates/luna-aot/src/embed.rs`
  fixed; 1 stale anchor in `docs/threading.md` fixed.
- `.lycheeignore` configured for pre-publish `docs.rs/luna-*`
  redirects.

### Phase 4 — PUC bytecode polish punts collapsed (PU)

PU audit identified 24 polish punts across 5.1/5.2/5.3/5.5 (5.4
already punt-free at v1.3 ship). Wave 1 extracted three shared
helpers (`lower_k_via_tmp` / `lower_i_imm` / `scan_tforprep_sites`)
to `crates/luna-core/src/vm/dump/puc/mod.rs`. Waves 2-4 collapsed
the punts dialect-by-dialect:

- **5.1**: PC remap upgraded to bidirectional (modeled on `puc_54.rs`),
  then 7/7 punts collapsed — SETLIST C=0 / arith RK-on-B (12 ops via
  `lower_k_via_tmp`) / EQ/LT/LE RK / LOADBOOL true+skip (via
  `LoadTrue + Jmp+1` pair through PC remap) / fb2int NEWTABLE hint /
  TFORLOOP N-way split (lower to `TForCall + TForLoop` via new
  `JumpKind::TForLoop` fixup; `A` direct = iter_base, differs from
  5.3) / LUAI_COMPAT_VARARG (runtime cold-path at `exec.rs:4200`
  already in v1.3; Wave 4 added the E2E test).
- **5.2**: 9 cases across 3 categories collapsed — arith K-on-LHS /
  arith K-on-both (inline pair) / EQ/LT/LE K (inline, since luna's
  `Op::Eq/Lt/Le` `k` bit is sense not constant flag) / GETTABUP
  register key (inline `GetUpval + GetTable` pair). 5.2 now
  punt-free.
- **5.3**: All 4 punts collapsed — generic-for (`TFORCALL + TFORLOOP`
  with `A = iter_base + 2 → iter_base` conversion since 5.3 lacks
  `OP_TFORPREP` but no TBC machinery either) / arith RK-on-B (12 ops
  via PC remap + helper) / LOADBOOL true+skip (Jmp pair through
  Fixup channel) / CONCAT B != A (Move-then-Concat pair with
  overlap-safe direction). 5.3 now punt-free except `OP_JMP A!=0`
  close-upvals (out of original 4-punt audit scope).
- **5.5**: 8/8 I-imm ops collapsed — ADDI / SHRI via `lower_i_imm`;
  SHLI / EQI / LTI / LEI / GTI / GEI inline (different shapes than
  `lower_i_imm`'s arith template). 5.5 now punt-free.

luna now loads `.luac` files from PUC 5.1 through 5.5 (and MacroLua)
without silent miscompile across the previously-punted opcode shapes.

### Phase 5 — Measurement-first baselines + documentation floor

#### Memory (MM)

- `dhat` dev-dep + `crates/luna-core/benches/mem_baseline.rs`
  exercising 5 workloads (cold_start / repl_idle / host_roots_churn /
  alloc_collect / userdata_lifecycle). Baseline snapshots at
  a local memory baseline.
- luna-core prod 0-dep contract preserved via `--edges normal` flag
  on `cargo tree`.
- Surprising finding: `TraceRecord::start` allocates ~557 KB across
  68 sites in `userdata_lifecycle` — confirms audit R2 (MM #5
  TraceRecord shrink blocked on Track R IR shape).
- Newly-surfaced attack candidate: `Vm::gc_roots` snapshot vec
  reallocs every GC (198 KB / 218 allocs in `alloc_collect`) —
  reusable.

#### Disk + binary size (DS)

- Local baselines covering per-crate
  package sizes, AOT output binary sizes (3 representative scripts ×
  3 build profiles), Mach-O section breakdown, runtime-helpers
  staticlib/rlib. Zero material drift from v1.3 audit values.
- 11 budget proposals with feasibility tags. AOT slim-profile output
  ≤ 3.7 MiB stripped tagged HIGH effort (requires both
  `panic="abort"` and Cranelift `all-arch` opt-out, both gated
  breaking changes).

#### Coverage (CV) gap fill

- 38 new tests + 1 new CI job (`send-feature`) across the audit's
  top-5 coverage gaps: async_drive (5 tests) / pattern engine (12
  tests) / aot_meta walker error paths (10 tests) / luna-jit-derive
  direct unit tests (11 tests, via inline `#[cfg(test)] mod`
  reaching private fns without `pub(crate)` hatches) / send_vm
  feature-matrix CI (8 SendVm tests already existed behind
  `#[cfg(feature="send")]`, no CI job exercised them).
- Zero real bugs surfaced.

#### Docs (DO) — 6 industrial-grade docs landed

- `docs/security.md` — threat model + sandbox boundaries.
- `docs/migration-v1-to-v2.md` — scaffold with TBD placeholders
  per breaking-change category, fills land at ship.
- `docs/aot.md` — AOT single-binary deploy guide (when / how /
  cross-compile / size breakdown / limitations / inspection).
- `docs/deploy.md` — production deployment patterns
  (crate selection / packaging shapes / runtime knobs / observability
  / graceful shutdown / cross-thread).
- `SECURITY.md` — formal CVE disclosure policy (email
  `admin@golia.jp`, 90-day default window).
- `CONTRIBUTING.md` — formal no-external-contrib policy
  (single-maintainer; PRs closed without review; fork freely under
  MIT/Apache-2.0).
- `docs/embedding.md` `vm.open_io()` / `vm.open_os()` stale API
  references corrected to `vm.open_os_io()`.
- `docs/architecture.md` crate layout refreshed from v1.1's 2-crate
  table to current 5 publishable + 2 dev-only; steel-cement-stone
  classification updated with actual file paths and v2.0 sprint
  discipline anchors.

#### AOT polish 6 verdict (AO-PF)

- Runtime counter added to chain reloc fire path
  (`crates/luna-runtime-helpers/src/lib.rs`).
- JIT in-process fib(28): **162,851 fires / 434,279 dispatches** —
  Stage 7 polish 6 alive on the JIT side.
- AOT-binary workload battery (fib(20), sum(1000), inlined helper,
  counted loop, GetField loop): **0 fires across all 5** — Stage 7
  polish 6 effectively dead in AOT, **but not the polish itself**:
  the AOT recorder filter (`dispatchable=false` for self-recursive
  traces) keeps input from ever reaching it. Verdict + handoff at
  the AO-PF verdict. **Not reverted** (JIT side
  active); recorder fix deferred to Track R landing.

---

## [1.3.0] — 2026-06-25

> **Released** — 2026-06-25 to crates.io. All five workspace crates
> shipped at `= 1.3.0`:
> [`luna-core`](https://crates.io/crates/luna-core/1.3.0) ·
> [`luna-jit-derive`](https://crates.io/crates/luna-jit-derive/1.3.0) ·
> [`luna-jit`](https://crates.io/crates/luna-jit/1.3.0) ·
> [`luna-runtime-helpers`](https://crates.io/crates/luna-runtime-helpers/1.3.0) ·
> [`luna-aot`](https://crates.io/crates/luna-aot/1.3.0). GitHub
> release: <https://github.com/goliajp/luna/releases/tag/v1.3.0>.

**Mega sprint** — 2026-06-24 user directive collapsed the planned
v1.2.0 + v1.3.0 + v1.4.0 + parts of v2.0 into a single ship under
the `nodefer` upgrade ("nothing is deferred to v1.4 or later").
Headline phases:

- **Phase A** (was v1.2): `LuaUserdata` trait sugar, REPL multi-line
  + history, lint debt cleared, Track B/L/P/R/S/G floor — already
  on develop (commits `bc088bd` / `65ca2cc` / `70c4bff`).
- **Phase B-N** (v1.3 expanded): PUC luac body 5.1-5.5, Send-safety
  full impl, perf attack round 2 (Path B math-fold extend), wasm32-
  wasip1 port, true `obj.x` field-style + `derive(LuaUserdata)`,
  REPL tab + syntax highlight, async natives in dispatch, userdata
  Trace-bearing host payloads, host_roots slot recycling, **luna-aot
  native-binary compile**, **MacroLua dialect support**.

See the v1.3 charter for the full track list, time
window estimate, and Phase ordering. `nodefer` is the operating
contract: every line item ships in v1.3 or is documented as
permanently out-of-scope (currently only the `luna` crates.io name
reclaim falls there — sticking with `luna-jit`).

The Phase A content below was previously written under the
`[1.2.0]` heading; it ships now as part of v1.3.0 without a
separately-published v1.2.0 on crates.io.

### Phase A headline

Polish + ergonomics on the v1.1 ship. **`LuaUserdata`
trait sugar** for Lua-callable host types, REPL gets multi-line input
plus history, lint debt cleared, perf attack discovers the real
bottleneck (interp, not trace) and updates the methodology accordingly.

### Track B — `LuaUserdata` trait (new embedder surface)

- **`luna_core::vm::userdata_trait`** module exposes the
  [`LuaUserdata`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/trait.LuaUserdata.html)
  trait + [`UserdataMethods<T>`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/trait.UserdataMethods.html)
  builder + [`MetaMethod`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/enum.MetaMethod.html)
  enum. Embedders register methods (`add_method` / `add_method_mut`),
  static fns (`add_function`), metamethods (`add_meta_method`), and
  call-syntax field getters (`add_field_method_get`) via a typed
  builder.
- **Per-Vm metatable cache** keyed by `TypeId::of::<T>()`. First
  `create_userdata::<T>` triggers `T::add_methods` once; subsequent
  instances reuse the cached `Gc<Table>`. Pinned via `pin_host` so
  GC keeps the metatable live.
- **`Vm::create_userdata` / `Vm::set_userdata` bound tightened** from
  `T: Any + 'static` to `T: LuaUserdata`. **BREAKING**: existing
  B8 users upgrade with `impl LuaUserdata for MyType {}` (one line).
- **Auto-install metatable + `__gc` finalizer wire** at userdata
  allocation time (`check_finalizer_userdata` called from
  `create_userdata`).
- **`FromLuaArgs::from_lua_args_skip_self`** added — the
  method-call shape where slot 0 is the receiver.
- **`FromLuaArgs for Vec<Value>`** — variadic decoder for
  dispatcher-style natives (e.g. `redis:call(cmd, ...)`).
- Three new runnable examples:
  `examples/userdata_demo.rs` (Counter), `userdata_vec3.rs`
  (arithmetic metamethods), `userdata_redis_stub.rs` (dogfood §4.1
  shape — state IS the payload, no `thread_local!`).
- `docs/embedding.md` §7 rewritten with subsections covering trait
  shape, static constructors, variadic dispatch, the v1.2 field-style
  limitation (call-syntax only — true `obj.x` deferred to v1.3, see
  Deferred section), GC ordering, and trait contract reminders.

### Track R — REPL

- **Multi-line continuation**: incomplete statements (detected via
  `SyntaxError::msg.contains(" near <eof>")`) emit `>>` and accept
  another line. `local x = function()` + `return 1` + `end` now
  works at the REPL instead of erroring on line 1.
- **`~/.luna_history` persistence**: 1000-entry capped history,
  loaded on startup, saved on exit. No new dependency
  (`std::env::var_os("HOME")` only).

### Track L — Lint debt cleared

- `cargo fmt --all` clean (cleared 606-site formatter drift from v1.0/v1.1).
- `cargo clippy --workspace --all-targets -- -D warnings` clean
  (12 historic errors fixed: 8 `not_unsafe_ptr_arg_deref` justified
  with rationale, 2 `approx_constant` → `std::f64::consts::PI`,
  1 ZST `uninit_assumed_init` constant-folded guard, 2 dialect-test
  fixture allows).
- `cargo fix` unused-imports sweep across 60+ files plus 5 hand-fixed
  clippy issues (unnecessary unsafe, match→unwrap_or_default, etc.).
- Workspace `[lints.clippy]` policy in `Cargo.toml` declares the
  strict baseline and the few documented exemptions
  (`missing_safety_doc` — `docs/unsafe-accounting.md` is SoT;
  `incompatible_msrv`, `too_many_arguments`, etc.).

### Track P — Perf attack (real bottleneck identified)

- **D2 criterion infra** + Linux CI runner workflow_dispatch
  perf-gate (manual trigger; `redis_lua_shape` baseline).
- **D3 TA1 Path B lowerer**: `GetTabUp` admitted into the trace
  recorder as a standalone helper (was: unconditional bail at
  `trace.rs:3030`). Traces compile end-to-end on the token-bucket
  shape; bail rate 0.
- **D4 A1 GetField fast path**: `Table::get_str` + Op::GetField
  interp arm skip `op_index` when the receiver is a known `Value::Str`
  with no metatable (commit `a2c98ae`).
- **`Vm::current_op`** API (ergo.rs) + `diag_opcode_breakdown.rs`
  example — runtime opcode counter for `[perf-decomposition-vs-polish.md]`
  §2 Phase A "actual workload validates the decomp" hard gate.
- **Methodology lesson** (`docs/performance.md` + global
  methodology doc updated): the v1.0 charter hypothesis "1.5×
  gap vs PUC 5.1 on token_bucket" was 4× understated. PUC 5.5 is
  ~4.1× faster than luna interp on the shape; LuaJIT 2.1 is ~196×.
  **True attack surface = interp,  not trace.** Trace JIT does not
  engage on the Redis-Lua-shape workload (`infer_getx_exit` returns
  None on the `Call(Native math.min)` mid-body; length-gate kicks in
  on short bodies). D4 A3/A4/A5 + Path B math-fold extend recorded
  as Deferred-to-v1.3 (NOT silent — see Deferred).

### Track S — `feature = "send"` framework reserved

- `[features] send = []` declared in `crates/luna-core/Cargo.toml`.
  Building with `--features send` triggers `compile_error!` pointing
  to `v1.3-rfc-send-arc.md`. Embedders can feature-detect (`cargo
  add luna-core --features send` fails loudly) without waiting for
  the v1.3 implementation.
- Phase 0 audit (`v1.2-audit-send-cost.md`): ARM M-series ~10%
  overhead (within RFC 15% ceiling); x86_64 Linux ~20% (refines the
  RFC ceiling, needs `SendVm` newtype fork in v1.3).

### CI / release infra (Track G)

- **Lint gate**: `cargo fmt --check` + `cargo clippy --workspace
  --all-targets -- -D warnings` on every push.
- **0-dep gate**: `cargo tree -p luna-core --prefix none` must show
  exactly one line (luna-core itself). Catches accidental
  dependency creep at PR time.
- **Unsafe-drift gate** (new in v1.2): first-party unsafe site count
  must stay under a recorded ceiling (490, baseline 461 from v1.1
  + ~15 from Track B). Bump the ceiling explicitly when justified;
  never widen to silence drift.
- `branches: [main]` → `[master, develop]` to track git-flow setup.
- `docs/release-checklist.md` (new) — version-agnostic checklist
  template; sprint-specific audits stay in the maintainer's local area.
- A discussion note archives the
  v1.1.0 ship-time rename story (`luna` → `luna-jit`).

### Phase B-N — v1.3 expansion in flight

Per the 2026-06-24 `nodefer` directive every item below is **in
scope** for v1.3 (no longer deferred). Tracked in
the v1.3 charter and plan state:

- **Path B math-fold extend** (`min` / `max` 2-arg) — *(landed Phase P2A)*
  `trace.rs::try_match_trace_math_fold` extended with `FoldKind::Min2 /
  Max2`. Split-window recognizer (only `GetTabUp + GetField + Call`
  flagged in `folded_ops` — arg-prep ops execute normally). Cranelift
  `smin/smax` for Int/Int, `fmin/fmax` for Float-or-mixed.
  `trace_dispatched_count` flipped 0 → 200/200 on `diag_token_bucket`.
  **TA3 default flip done** — `jit_state.rs::with_null_backend` ships
  `trace_enabled = true` (was `false`) after Linux taskset perf-gate
  confirmed `redis_lua_shape ≥ 1.0×` v1.2 baseline. Embedders that want
  the v1.2 interp-only default call `vm.set_trace_jit_enabled(false)`.
- **D4 A3 / A4 / A5** (newindex double-walk collapse / Move
  elimination / dispatcher reshape) — perf polish on top of A1.
- **`add_field_method_set` + true `obj.x` field-style access** —
  *(Phase UD1+UD2 landed)* `add_field_method_set(name, fn)` registers
  setters for `obj.name = value`; the `__index` slot becomes a native
  trampoline when any getter is registered, so `obj.width` (no parens)
  resolves to the field value directly. **Breaking change** from the
  v1.2 sugar: `obj:name()` call-syntax for `add_field_method_get` no
  longer works (the trampoline calls the getter and returns
  `Int(...)`, so `Int(...)(obj)` errors). Embedders who need both
  shapes should register an explicit `add_method("name", ...)`
  alongside the field-getter. Unknown writes go to a runtime error
  rather than silently dropping.
- **`#[derive(LuaUserdata)]` proc-macro** — *(Phase UD3 landed)* new
  `luna-jit-derive` crate ships the derive + `#[lua_userdata_methods]`
  attr macro. Helper attributes: `#[lua_method("name")]`,
  `#[lua_method_mut]`, `#[lua_function]`, `#[lua_meta_method(Add)]`,
  `#[lua_meta_method_mut]`, `#[lua_field_get]`, `#[lua_field_set]`,
  `#[lua_skip]`, plus struct-level `#[lua_type_name = "X"]`. Hand
  impl stays as the escape hatch (generic types, conditional method
  sets). luna-core 0-dep contract preserved — derive lives in
  `luna-jit-derive` only; luna-jit's build-time supply chain grows by
  `syn + quote + proc-macro2` (the standard derive trio). `cargo
  tree -p luna-core --prefix none --no-default-features` still
  reports 1 row. Embedders writing `use luna_jit::LuaUserdata;` get
  both the trait (via the `pub use luna_core::*;` re-export) and the
  derive (`pub use luna_jit_derive::LuaUserdata;`).
- **`feature = "send"` real implementation** *(Phase SS-B landed)*
  — new opt-in cargo feature on luna-core (`send = []`) and
  luna-jit (`send = ["luna-core/send"]`) surfaces a second public
  type `luna_core::vm::SendVm` for cross-thread embedding. Shape:
  `SendVm { inner: Arc<UnsafeCell<Vm>>, lock: Arc<RwLock<()>> }`
  with `unsafe impl Send for SendVm` (justified by a runtime
  single-mutator invariant the lock re-establishes). Default-feature
  builds are bit-identical with the pre-SS-B baseline — bare `Vm`
  stays `!Send + !Sync` and pays no overhead. luna-core 0-dep
  contract preserved (`Arc`, `UnsafeCell`, `RwLock` are all
  stdlib).
  - **API surface mirror**: `eval`, `call_value`, `set_global`,
    `set_userdata`, `intern_str`, `open_base / open_math /
    open_string / open_table / open_coroutine`, the Phase SR
    `pin_host / read_host / unpin` host-roots methods, plus
    `Clone` (cheap — two `Arc::clone`), `Debug`, and one new
    method `get_global(name) -> Value` that isn't present on bare
    `Vm` (introduced because the bare `globals()` + raw `Gc<Table>`
    deref is awkward across the lock boundary).
  - **Interp-only constraint**: `SendVm::new` calls
    `Vm::new_minimal` which leaves `JitState` at `NullJitBackend`.
    The trace JIT does not run on a SendVm in v1.3. JIT-aware
    SendVm is a documented post-v1.3 polish item (the
    `Proto::traces: RefCell<Vec<Rc<CompiledTrace>>>` field
    intersects with `Send` and would need an `Rc → Arc` migration;
    audit projects ~6 % additional JIT-engaged cost). Not a
    defer — the v1.3 charter explicitly scopes interp-only as the
    SS-B deliverable.
  - **Cost** (macOS M-series, SS-B bench): SendVm pays ~+1.86 %
    token-bucket regression vs interp-only baseline `Vm` (175.46
    µs vs 172.26 µs). Better than the audit's projected ~3 % ARM.
    Linux x86_64 numbers land via the `perf-gate` CI matrix
    (audit projects ~6 %).
  - **8 smoke tests** in `crates/luna-core/tests/send_vm.rs`
    (gated `#[cfg(feature = "send")]`): compile-time `Send`
    assertion, basic eval, `thread::spawn` move, 100-thread
    concurrent contention (verifies serialized counter = 4950),
    userdata round-trip, HostRootTicket round-trip across the
    lock, pin-across-clones, and interp-only loop sum.
  - **Bench update** (`crates/luna-jit/benches/bench_send_overhead.rs`):
    feature-gated `send_vm_eval` and `send_vm_token_bucket` pairs
    added alongside the SS-A `wrapped_vm_*` NoOpWrapper baseline;
    apples-to-apples interp-bare counterparts (`bare_vm_interp_*`)
    added for the SendVm comparison.
  - **Documentation**: `docs/threading.md` gains a `SendVm`
    section covering when to use vs not, the shape + soundness
    story, the interp-only constraint, and a tokio multi_thread
    embed example (without depending on tokio in luna-core).
  - **Design RFC** documents
    the as-shipped wrapper choice + the decision to defer the
    audit's per-field `SendGc<T>` fork to v1.4+.
  - **Unsafe drift**: +5 first-party `unsafe` sites (480 → 485,
    ceiling 490 — 5 slots free). New sites: `unsafe impl Send for
    SendVm` (one), `&mut *UnsafeCell::get()` inside
    `with_vm_mut` (one), `(*globals.as_ptr()).get(key)` in
    `get_global` (one), two doc-comment occurrences caught by the
    grep regex.
  - **BREAKING vs v1.2 stub**: the v1.2 `[features] send = []`
    that raised a `compile_error!` when selected now compiles
    cleanly and surfaces `SendVm`. Embedders who were guarding
    against the compile_error with `cfg(not(feature = "send"))`
    no longer need that guard.
- **REPL C3 tab completion + syntax highlight** — `[features]
  repl-line-editor` (rustyline) non-default cargo feature.
- **PUC luac body 5.1-5.5** — full binary compat across all
  shipping Lua dialects; opt-in `Vm::set_puc_bytecode_loading(true)`
  + per-dialect translator under `crates/luna-core/src/vm/dump/`.
- **wasm32-wasip1 support** — `io.popen` / `os.execute` cfg-gated
  + wasi stubs return PUC error tuple.
- **`official_run` flakiness fix** — compiler short-circuit AND
  `debug_assert_eq!(reg, base)` + sweep misaligned-pointer cascade
  root cause + fix.
- **Async natives in dispatcher** (B11 hook firing) *(Phase AS
  landed)* — close the v1.1 B10 Stage 2 deferred path so async-marked
  natives compose with Rust-side `[B11]` debug hooks. Audit
  showed the gap was
  narrower than the v1.1 charter assumed: the dispatcher hot loop's
  `Count` / `Line` / Lua-`Call` / Lua-`Return` sites are opcode-driven
  and already fire correctly under `async_mode = true`; only the
  async-native call boundary itself was missing. Phase AS adds:
  - `Call` event on the async-native branch in
    `crates/luna-core/src/vm/exec.rs`, fired after the
    `native_nresults` / `gc_top` pin and before the future is built —
    same placement-relative-to-pin as the sync native path's
    `hook_call(true, nargs)` site (audit §A.1 / Q6).
  - `Return` event in `Vm::commit_async_native_result`
    (`crates/luna-core/src/vm/async_drive.rs`), fired after
    `finish_results` lands the resolved nret into the call window and
    before the post-call GC checkpoint. Mirrors the sync native's
    `hook_return(true, nargs + 1, nret)` placement. The method is
    now fallible — `EvalFuture::poll`'s `Poll::Ready(Ok(nret))` arm
    propagates the hook error through the same JIT-restore + cleanup
    path the `Poll::Ready(Err)` arm already runs.
  - **Count + Line carryover** — no code change; the dispatcher's
    persistent `hook.count_left` and `hook_lastline` `Vm` fields
    already carry across `Poll::Pending` returns to the executor, so
    a 1000-instruction count budget walks down naturally across
    arbitrarily many slice boundaries and a line event won't
    double-fire on resume mid-line. New tests pin both as regression
    guards.
  - **6 smoke tests** in
    `crates/luna-core/tests/async_hook_composition.rs`: `Call`/`Return`
    around an immediate-Ready async native, `Call`/`Return` bracketing
    a yield-once async native (proves the Return fires after `.await`
    resolves), count-hook carryover across an aggressive 50-op slice,
    line-hook dedupe across a 3-op slice, compile-time
    `assert_send::<RustDebugHook>()` + `assert_sync::<RustDebugHook>()`
    pinning the function-pointer Send-safety property, and a
    composition smoke confirming the hook body observes the
    async-native Call event end-to-end. No tokio dep — same
    hand-rolled `block_on` + `YieldOnce` harness as the existing
    `tests/async_native.rs` (luna-core 0-third-party-dep contract
    preserved).
  - **`Send` composition with SS-B** — `RustDebugHook = fn(&mut Vm,
    RustHookEvent)` is a bare function pointer and unconditionally
    `Send + Sync`, so the v1.3 Phase SS-B `SendVm` newtype composes
    cleanly with async hooks without any new trait bound. The
    compile-time `assert_send` test is the regression guard for any
    future evolution of the hook signature toward closure state.
  - **Re-entrancy contract**: hook bodies under async mode may call
    sync `vm.eval(...)` but must NOT invoke async natives — the
    inner sync `eval` lacks an executor to drive a nested
    `EvalFuture`, and the existing rejection
    (`"async native called in sync context"`) catches the attempt
    cleanly. Documented in `docs/threading.md` §"Async natives +
    debug hooks".
  - **Q5 followup** (audit §"Open questions"): `EvalFuture::Drop`
    already clears `pending_async_native_fut` /
    `pending_async_native_ctx` (`async_drive.rs:553-554` in the
    pre-AS code), so the stale-ctx hardening the audit flagged is
    already in place — no additional cleanup required in Phase AS.
  - **Unsafe drift**: 0 new sites. Hook visibility bump from `fn`
    to `pub(crate) fn` on `Vm::hook_call` and `Vm::hook_return` is
    safe-Rust-only.
- **Userdata `Trace`-bearing host payloads** — `T` may hold
  `Gc<...>` fields; collector recurses into the payload (userdata
  GC ripple).
- **`host_roots` slot recycling** *(Phase SR landed)* — the v1.1
  append-only `Vec<Value>` is replaced by a free-list-backed slot
  pool keyed by `HostRootTicket { idx: u32, generation: u32 }`
  (8 bytes, `Copy`). `pin_host` returns the ticket; `unpin` clears
  the slot to `Nil`, bumps generation, and pushes the index onto
  the free list for reuse; `read_host` / `write_host` validate the
  ticket's generation and return `None` / `Err(HostRootStale)` on
  stale lookup (ABA-safe). Generation overflow at `u32::MAX` retires
  the slot permanently (bounded leak: ~4 days at 10⁹ unpins/day per
  slot). Long-running embedders (request-per-script loops, edge
  workers) now hold at a bounded pool size instead of growing the
  vector monotonically.
  - **BREAKING — embedder Vm API**: `Vm::pin_host(v: Value) -> usize`
    is now `Vm::pin_host(v: Value) -> HostRootTicket`;
    `Vm::host_root_at(idx) -> Value` and `Vm::host_root_set(idx, v)`
    are **removed** in favor of `Vm::read_host(t) -> Option<Value>`
    and `Vm::write_host(t, v) -> Result<(), HostRootStale>`. New
    methods: `Vm::unpin(t) -> Result<(), HostRootStale>`. Existing
    `Vm::unpin_all()` and `Vm::host_root_count() -> usize` signatures
    unchanged; `unpin_all` semantics extended to bump every slot's
    generation (all outstanding tickets become stale uniformly).
    Migration: replace stored `usize` index with `HostRootTicket`;
    `vm.host_root_at(idx)` → `vm.read_host(ticket).expect("...")`;
    `vm.host_root_set(idx, v)` → `vm.write_host(ticket, v).unwrap()`.
  - **BREAKING — `luna-jit` facade structs**: `LuaFunction` /
    `LuaTable` / `LuaRoot` now carry `ticket: HostRootTicket`
    (was `idx: usize`). `Copy + Clone` preserved; public method
    surface (`call` / `call_multi` / `get` / `set` / etc.) is
    invariant. New `Lua::unpin(handle)` releases a single handle
    via the new `PinnedHandle` trait (impl'd by all three handle
    types). Reads after `Lua::unpin` / `Lua::unpin_all` panic with
    `"<HandleType> used after unpin / unpin_all"` — matches the v1.1
    "handles created before `unpin_all` become invalid" docstring.
  - New module: `luna_core::vm::host_roots` (own the pool impls);
    types re-exported as `luna_core::vm::{HostRootTicket, HostRootStale}`.
    Tests: `crates/luna-core/tests/host_roots_slot_recycling.rs`
    (10 tests covering basic recycle, ABA detection, `unpin_all`
    invalidation, 100k pin/unpin smoke, free-list LIFO, GC tracer
    correctness across recycle).
- **`luna-aot` native-binary compile** *(Phase AOT scaffold landed;
  Cranelift codegen follow-up within v1.3)* — new sibling crate
  `crates/luna-aot/` (workspace member alongside `luna-core` +
  `luna-jit` + `luna-jit-derive`). Ahead-of-time compiler that
  emits a self-contained binary embedding the Lua bytecode with no
  runtime parse step.
  - **Scaffold pipeline end-to-end** today: Lua source →
    `luna_core::frontend::parser::parse` → AST →
    `luna_core::compiler::compile_chunk` → `Gc<Proto>` →
    `luna_core::vm::dump::dump` → luna body dump bytes →
    `object::write::Object` with a `.luna.bytecode` ReadOnlyData
    section bracketed by global symbols
    `__luna_bytecode_start` / `__luna_bytecode_end` (Mach-O
    `_`-prefixed) → system `cc` link with a minimal C entry +
    bytecode `.o` → host-triple native binary that prints the
    embedded section length to `stderr` (proves the section is
    reachable end-to-end).
  - **CLI**: `luna-aot compile <input.lua> [--out <path>]
    [--target <triple>] [--dialect 5.1|5.2|5.3|5.4|5.5|macrolua]`.
    `clap` derive surface; scaffold rejects non-host `--target`
    until Stage 6 cross-compile lands.
  - **Library surface**: `luna_aot::embed::embed_bytecode(source,
    out, target_triple, version)` for programmatic embedders;
    `luna_aot::runtime_stub::aot_main()` (interp-driven Vm
    entry — compiles cleanly, awaiting wire-up to the link step in
    the follow-up session via cargo-bootstrap or staticlib
    distribution per audit § Stage 6 Option A/B); constants
    `BYTECODE_START_SYMBOL` / `BYTECODE_END_SYMBOL` /
    `BYTECODE_SECTION_NAME`.
  - **Supply-chain delta**: `luna-aot` pulls `object 0.36`
    (`default-features = false`, `elf` + `macho` + `pe` + `write_std`)
    + `clap 4` (derive) + dev-only `tempfile 3`. **luna-core
    0-third-party-dep contract is unaffected** — `cargo tree -p
    luna-core --prefix none --no-default-features | grep -cE " v[0-9]"`
    still reports 1. Workspace-wide transitive growth = ~50 crates
    (clap + object + their derive transitives). cargo-deny config
    may want a `[bans] multiple-versions = "warn"` pass; flagged
    for the follow-up phase that adds the per-crate deny job.
  - **Test**: `crates/luna-aot/tests/scaffold_smoke.rs` exercises
    the end-to-end path (parse + compile + dump + `.o` write + `cc`
    link → on-disk non-empty native binary). Does not execute the
    binary — the scaffold's C entry's stderr-only output isn't a
    correctness signal for this session; the runtime-stub follow-up
    adds the stdout-comparison test.
  - **Phase AOT Stage 3 — backend-agnostic lowerer** *(landed in
    this commit)*. Both `lower_int_chunk_into<M: Module>` and
    `lower_trace_into<M: Module>` in `luna-jit::jit_backend` are now
    generic over `cranelift_module::Module`, so the same codegen
    body drives the runtime `JITModule` (live RWX mmap) and the AOT
    `ObjectModule` (`.o` file emission). The JIT-specific module
    construction (`JITBuilder::with_isa` + `builder.symbol("luna_jit_*",
    …)`) is factored into thin helpers `build_jit_module_with_helpers`
    (int-chunk) + `build_trace_jit_module` (trace), keeping
    `JITModule::finalize_definitions` /
    `get_finalized_function` / `TRACE_JIT_HANDLES` insertion isolated
    in the JIT wrappers. The two trace-lowering free fns
    `emit_table_set` / `emit_materialize_live_sunk` are now also
    generic over the module trait. Trace returns place a
    `placeholder_trace_fn` in `CompiledTrace.entry`; the JIT wrapper
    patches the real fn pointer after finalize, while the AOT
    pipeline resolves the symbol at static-link time and never
    invokes `entry` directly. A new smoke test
    `crates/luna-aot/tests/stage3_lower_into_object.rs` drives the
    int-chunk lowerer with `cranelift_object::ObjectModule` and
    asserts the produced bytes carry the host's object-file magic
    number — load-bearing witness that the generic boundary is
    actually consumed by a second backend, not just claimed.
    Helper-symbol registration is JIT-only for now; the AOT pipeline
    will resolve these via static link against a small
    `luna-runtime-helpers` rlib in a follow-up (audit § Stage 3
    Action item 3). 274 / 274 workspace lib tests + 360+ luna-jit
    integration tests stay green; the pre-existing
    `trace_jit_s1` failures (2 / 4, baseline-drift from the TA3
    `trace_enabled = true` ship default) and the known
    `official_run` SIGABRT (IO Safety fd-double-close, see
    the IO-safety known bug) are unchanged
    by this refactor.
  - **Phase AOT Stage 4 — linker + interp-runtime staticlib**
    *(landed in this commit)*. A new sibling crate
    `crates/luna-runtime-helpers/` ships as a dual
    `crate-type = ["staticlib", "rlib"]` library that depends only
    on `luna-core` (luna-core's 0-third-party-dep contract is
    unaffected — `cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"`
    still reports 1). It exposes one C-ABI symbol
    `#[unsafe(no_mangle)] pub unsafe extern "C" fn luna_aot_run(bytecode: *const u8, len: usize) -> i32`
    that constructs a `Vm::new(LuaVersion::Lua55)`, enables
    bytecode loading, calls `Vm::load(slice, b"=embedded")`, runs
    `call_value` on the root closure, and returns the process exit
    code (0 success / 1 load-or-runtime-error / panics caught and
    reported). The new `luna_aot::embed::compile_and_link`
    function in `crates/luna-aot/src/embed.rs` drives the full
    deploy pipeline: parse → compile → dump → bytecode `.o` → C
    `main.c` (extern-decls the bracket symbols + `luna_aot_run`,
    emits `cc -c main.c -o main.o`) → `cargo build -p
    luna-runtime-helpers --release` (or `LUNA_AOT_RUNTIME_HELPERS_STATICLIB`
    env override for distribution scenarios; in-process `Mutex`
    serialises concurrent in-test callers against cargo's atomic-
    rename window) → `cc bytecode.o main.o libluna_runtime_helpers.a
    [platform libs] -o <out>` (mac: `-framework CoreFoundation
    -framework Security -liconv`; linux:
    `-lpthread -ldl -lm -lrt -lgcc_s -lutil`; windows: explicit
    `AotError::Link` — Windows folds into the cross-compile
    follow-up). The CLI's `compile` subcommand routes through
    `compile_and_link` by default; the prior scaffold path
    (C-entry-only, prints section length to stderr) remains
    reachable via `--scaffold-only` for users who want to
    benchmark the link step in isolation. New test
    `crates/luna-aot/tests/stage4_link_and_run.rs` covers three
    end-to-end scenarios: `print('hello from aot')` lands on
    stdout with exit 0; arithmetic + multi-print
    (`print(5); print('done', 10)`) produces the expected
    tab-separated PUC-shape output; `error('boom')` propagates as
    exit 1 with the message on stderr. Tests skip cleanly on
    Windows / missing-`cc` hosts (Stage 4 ships Unix-only).
    Stage 5 Cranelift trace-mcode emission and Stage 6
    cross-compile remain follow-ups.
  - **Phase AOT Stage 5 — cross-compile via `--target`** *(landed in
    this commit)*. New public `luna_aot::embed::TargetSpec` resolves
    a triple string into the per-target bundle the pipeline needs:
    `object` format (ELF / Mach-O / PE), arch, endianness, OS family
    (`TargetOs::{MacOs, Linux, Windows}`), libc flavour
    (`TargetLibc::{Default, Musl, MinGw}`), and the right `cc`
    driver. Resolution prefers a named cross-cc on PATH
    (`aarch64-linux-gnu-gcc`, `x86_64-w64-mingw32-gcc`,
    `x86_64-linux-musl-gcc`, ...) then falls back to `cc -target
    <triple>` (works on macOS hosts where Apple's clang accepts
    `-target` natively). `build_runtime_helpers_staticlib` now takes
    `Option<&str>` and shells out to
    `cargo build --target=<triple> -p luna-runtime-helpers --release`
    when a non-host triple is requested; the resulting staticlib
    lands at `target/<triple>/release/libluna_runtime_helpers.a`
    (or `luna_runtime_helpers.lib` on Windows). The final link uses
    a per-OS lib set: macOS keeps `-framework CoreFoundation
    -framework Security -liconv`; glibc Linux keeps the Stage 4
    `-lpthread -ldl -lm -lrt -lgcc_s -lutil` set; musl Linux drops
    `-lrt -lgcc_s -lutil` (those symbols are inside musl libc);
    Windows-MinGW adds `-luserenv -lkernel32 -lws2_32 -lbcrypt
    -ladvapi32 -lntdll` (the rust stdlib's win32 shim deps as
    reported by `rustc --print native-static-libs`). Tier 1
    (verified end-to-end on macOS aarch64 host): host triple +
    `x86_64-apple-darwin` cross. Tier 2 (codegen + link wired,
    self-skip when host cross-cc is missing):
    `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`,
    `x86_64-unknown-linux-musl`, `x86_64-pc-windows-gnu`. New test
    `crates/luna-aot/tests/stage5_cross_compile.rs` covers seven
    cases: pure-unit triple-parser smoke (`target_spec_parses_tier1_triples`),
    unsupported-arch rejection (`target_spec_rejects_unsupported_arch`),
    plus one `cross_compile_*` test per tier-2 triple. Each
    per-triple test reads the produced binary's leading bytes and
    asserts the object-file magic matches the requested format
    (ELF `\x7fELF`, Mach-O `0xfeedfacf` / `0xcffaedfe`, PE `MZ`).
    All tests self-skip with informative `eprintln!` lines when
    rust-std or cross-cc isn't installed; the test list is green
    on a generic dev box without any cross-toolchains.
  - **Phase AOT Stage 5 — Windows linker** *(landed in this commit)*.
    The Stage 4 hard error `"Windows linker support not implemented"`
    is replaced with two clear paths: MinGW
    (`x86_64-pc-windows-gnu` → `x86_64-w64-mingw32-gcc`) is wired
    through the regular target-aware `cc` driver pick + the
    Windows-MinGW lib set; MSVC (`x86_64-pc-windows-msvc`) returns
    `AotError::Link` with a concrete workaround message
    ("target `x86_64-pc-windows-gnu` instead, or run
    `--scaffold-only` and invoke link.exe by hand"). The MinGW path
    is exercised by `stage5_cross_compile::cross_compile_x86_64_pc_windows_gnu`,
    which self-skips when `x86_64-w64-mingw32-gcc` isn't on PATH.
  - **Phase AOT Stage 6 — Alpine no-Lua deploy smoke** *(landed in
    this commit)*. Charter AOT6 closure. New test
    `crates/luna-aot/tests/stage6_alpine_smoke.rs` builds
    `hello.lua` for `x86_64-unknown-linux-musl`, runs the
    resulting binary inside an `alpine:3.20` container with **no
    Lua installed** (no `apk add lua*`), and asserts stdout matches
    the expected `print(...)` output. A best-effort secondary
    `verify_only_musl_libc` step uses busybox `strings | grep` to
    confirm the binary doesn't reference `liblua` or `libluna`.
    Self-skips cleanly when any prerequisite is missing:
    docker/podman daemon (tries both), rust-std for the musl
    triple, musl cross-cc, network access to `docker.io`. The skip
    paths print one-line `eprintln!` install hints (`brew install
    FiloSottile/musl-cross/musl-cross` for macOS, `apt install
    musl-tools` for Debian).
  - **Final phase remaining**: trace JIT mcode emission via
    `cranelift-object` (walk every reachable `Proto`'s hot loops,
    drive each through the Stage 3 generic lowerer, emit symbols +
    dispatch table into the AOT binary). The interp staticlib
    runtime already carries the fallback so trace.o is purely
    additive; this is post-v1.3.
- **MacroLua dialect support** — Lua syntax extension as an
  optional dialect alongside 5.1-5.5; routed through the existing
  per-dialect lexer/parser machinery so it doesn't disturb the
  PUC compatibility matrix.

### Permanently out-of-scope (decision 2026-06-24)

- **Reclaim `luna` crate name on crates.io** — abandoned; sticking
  with `luna-jit` for the JIT-equipped crate and `luna-core` for
  the 0-dep interpreter. See
  a discussion note kept with the project's private records.

### Internal — sprint methodology

- The perf baselines from 2026-06-24 record the decomp work
  that surfaced "interp not trace" as the true attack surface.
- The perf-attack methodology gained an anti-pattern catalog drawn
  from the v1.0 fib_28 misdirection.
- Charter, plan-state and audit docs live in the maintainer's local area
  (gitignored); `docs/` stays user-facing.

---

## [1.1.0] — 2026-06-23

### Ship-time crate rename

The JIT-equipped crate is published as **`luna-jit`** instead of
`luna` because the `luna` name on crates.io is taken by an
unrelated utilities library. The directory layout, library
exports, and CLI binary name (`luna`) are unchanged; only the
crate name visible on crates.io is `luna-jit`. Embedders use:

```toml
[dependencies]
luna-jit = "1.1"   # or:   luna-core = "1.1"   for the 0-dep core
```

```rust
use luna_jit::Lua;   // (was `use luna::Lua;`)
```

The CLI binary still installs as `luna` (`cargo install luna-jit`
puts a binary named `luna` on PATH). `luna-core` keeps its name
(0-dep interpreter is the pure thing).

### Track A — Crate / Dep / Safety

- **Workspace split** (A1): `luna-core` (0 third-party deps; lexer /
  parser / compiler / interpreter / runtime / stdlib / GC / pattern /
  JIT trait surface) and `luna` (Cranelift JIT + capi + CLI binary).
  `cargo add luna-core` pulls only the interpreter; `cargo add luna`
  pulls the full JIT'd stack. CI gate: `cargo tree -p luna-core`
  must show exactly one crate.
- **JIT trait boundary** (A1 Session A): `IntChunkCompiler` /
  `TraceCompiler` traits in `luna_core::jit::abi` decouple the
  dispatcher from Cranelift. `NullJitBackend` (in `luna-core`) and
  `CraneliftBackend` (in `luna`) implement the traits.
- **`Vm::new_minimal_with_jit`** in the `luna` crate — one-line
  constructor for embedders wanting the v1.0 JIT-on-by-default
  behavior through `cargo add luna`.
- **`Vm` rustdoc + `!Send` compile_fail doctest** (A7) — `Vm: !Send + !Sync`
  is now CI-enforced. `docs/threading.md` covers canonical
  embedding patterns.
- **`JitState` sidecar** (A2): JIT-specific Vm fields factored into
  a dedicated struct, freeing the Vm hot path from JIT churn.
- **SAFETY: comment coverage** (A6): 100% across `unsafe { ... }`
  blocks. 342 new annotations added. See `docs/unsafe-accounting.md`.
- **Public API 0 unsafe** (A4): 4 `pub unsafe fn` items demoted to
  `#[doc(hidden)]`; `TableBuilder` / `IntoValue` / `native_typed`
  cover the safe embedder flows. The dogfood §4.1 friction is closed.
- **Panic-safe public boundaries** (A5): `Vm::set_global` returns
  `Result<(), LuaError>`; 68 call sites updated.
- **`cargo-deny`** (A3): CI workflow gates supply chain (advisories,
  licenses, source registry) plus a hard `luna-core` 0-dep check.

### Track B — Embedder API

- **`Vm::sandbox(version).build()`** (B1): Conservative-default
  sandbox builder; embedders whitelist stdlib modules + set
  instr/memory budgets in one chain.
- **`vm.eval` / `vm.eval_chunk`** (B2): Single-call source-to-value
  evaluation returning `Result<Vec<Value>, LuaError>`. SyntaxError
  surfaces as a heap-interned `LuaError`.
- **`TableBuilder` + `vm.table_of`** (B3): Build tables with chained
  `.with(k, v)` calls or a fixed-size slice. Embedders never write
  `unsafe { gc.as_mut() }` for table construction.
- **`IntoValue` trait** (B4): `vm.set_global("k", 42_i64)` infers;
  blanket impls cover `i64`, `f64`, `bool`, `&str`, `String`,
  `Vec<u8>`, `Gc<Table>`, `Gc<LuaClosure>`, `Gc<NativeClosure>`,
  `Value`, `()`, `Option<T>`.
- **`vm.native_typed` + `FromLuaArgs`/`IntoLuaReturn`/`FromLuaValue`**
  (B5): Typed Rust functions exposed as Lua callables. Arities 0-6,
  fn pointers and non-capturing closures, multi-value returns,
  `Result<T, LuaError>` for fallible natives.
- **Structured `LuaError`** (B6): Adds `LuaErrorKind` enum
  (Runtime / Syntax / InstrBudget / MemoryCap / Native /
  OutOfMemory / Type), `impl Display + Error` on `LuaError`,
  Vm-side `error_kind` / `error_source` / `take_error_traceback`
  accessors. `LuaError` stays `Copy`.
- **String interop** (B7): `vm.intern_str`, `Value::try_as_str`
  (UTF-8 validating), `Value::as_bytes` (binary-safe).
- **Host userdata** (B8): `vm.create_userdata::<T>(value)` /
  `set_userdata` / `userdata_borrow` / `Userdata::downcast` for
  arbitrary `T: 'static` host types. The closed-world userdata
  infrastructure now accepts host payloads.
- **Rust-side coroutine drive** (B9): `vm.create_coroutine` /
  `vm.resume_coroutine` parallel to `coroutine.create` / `:resume`.
- **Async embedder API** (B10): `vm.eval_async` returns a `!Send`
  Future driving the dispatcher with cooperative yields on
  instruction budget exhaustion. `vm.set_async_native` exposes
  async Rust functions to Lua scripts. `Lua::eval_async` /
  `Lua::set_async_native` mirror on the facade.
  `examples/async_host.rs` ships a runnable Tokio-substitute
  walkthrough. 0 new third-party deps (`std::future` + `std::task`
  suffice).
- **Rust-side debug hook** (B11): `vm.set_rust_debug_hook` accepts
  a `fn(&mut Vm, RustHookEvent)` plus mask flags
  (HOOK_MASK_CALL / RETURN / LINE / COUNT). Both Lua-side
  `debug.sethook` and Rust hooks can coexist.
- **`Lua` newtype facade** (B12): `mlua`-shape front door with
  owned `LuaFunction` / `LuaTable` / `LuaRoot` handles backed by
  an append-only `Vm::host_roots` pool. Use `Lua::new()` for the
  five-minute start; use `Vm` for the low-level handle.

### Track C — CLI / REPL

- **Interactive REPL** (C1): `luna` with no args drops into a
  single-line REPL. Each line is tried as an expression
  (`return <line>`), then as a statement on syntax error.
- **CLI flags** (C4): `--sandbox` builds via SandboxBuilder;
  `--budget=N` sets instr budget; `--no-jit` installs NullJitBackend;
  `--profile` prints trace-JIT counters on exit.
- **Pretty errors** (C5): Compile + runtime errors render with
  classified kind tag, source location, snippet, and traceback.
  ANSI color when stderr is a TTY and `NO_COLOR` is unset.

### Track D — Bench / Perf

- **Redis-Lua-shape micro-bench** (D1): New `redis_lua_shape` bench
  with four workload shapes from the dogfood report
  (`token_bucket_1k`, `sliding_window_500`, `method_dispatch_5k`,
  `string_ops_2k`).
- **`docs/performance.md` extension** (F4): D1 baseline added
  alongside the cross-dialect snapshot.

### Track E — Dialect / require / Compat

- **`docs/compatibility.md` extension** (E2): v1.1 luna-specific
  extension table + CLI options reference + REPL behavior.

### Track F — Docs

- `docs/architecture.md` (F5): crate layout + source classification
  + JIT pipeline + threading + sandbox.
- `docs/threading.md` (A7 artifact): `!Send` patterns + Tokio +
  async embedder API.
- `docs/embedding.md` (F1): 12-section embedder cookbook
  (install / hello / sandbox / globals / tables / native_typed /
  userdata / coroutines / debug hooks / errors / Lua facade /
  threading).
- `docs/binary-size.md` (G5): cargo-bloat snapshot
  (cranelift_codegen 45% / luna_core 25% / std 13%).
- `docs/unsafe-accounting.md` (G4): cargo-geiger companion;
  461 unsafe sites, 394 SAFETY-annotated, 6 pattern categories.
- README.md rewrite (F6): workspace + ergo + honest perf.

### Track G — CI / Release

- **MSRV declaration** (G1): `rust-version = "1.86"` in
  `[workspace.package]`; CI workflow `.github/workflows/msrv.yml`
  locks against it.
- **CI matrix** (G2): `.github/workflows/ci.yml` runs
  build/test/release/doc on Linux + macOS + Windows + wasm32
  (luna-core only). `cargo doc --workspace -D warnings` gate.
- **`cargo-deny`** (A3, listed above): supply-chain + 0-dep gate.

### Changed

- Source tree reorganization: `src/jit/trace.rs` (9483 LOC) split
  in place into `trace.rs` (Cranelift codegen body) and
  `trace_types.rs` (type definitions + thresholds + cranelift-free
  helpers). Type paths preserved via re-exports; downstream
  callers see no API change.
- `Vm::set_global` signature changed from
  `(&mut self, name: &str, v: Value)` to
  `<V: IntoValue>(&mut self, name: &str, v: V) -> Result<(), LuaError>`.
  Existing callers passing `Value::*` directly still compile (V
  infers to Value). New ergonomics: `vm.set_global("k", 42)`.

### Deferred to v1.2

- C2 (REPL multi-line continuation + history)
- C3 (REPL tab completion + syntax highlight, likely as
  `luna-repl` binary crate)
- D2 (criterion infra + n=1000 + CPU pin + 10 runs)
- D3 (token_bucket decomposition vs PUC 5.1)
- D4 (attack-agent perf workflow)
- E1 (require searcher table dispatch — behavior change requires
  PUC test re-verification)
- E3 (PUC `luac` body 5.1-5.5 compat — 20-30 day block, charter L)
- E4 (string.pack/utf8 edge case test gaps)
- Lint cleanup (`cargo fmt --all` 606 sites + 9 `clippy` errors,
  a known drift in historic fmt/clippy runs)
- `feature = "send"` `Arc<RwLock<T>>` sprint (see
  the v1.1 Send/Sync RFC)
- `LuaUserdata` trait sugar (B8 follow-on; closed-world ships
  v1.1, trait sugar lands later)

---

## [1.0.0] — 2026-06-23

First stable release. luna implements **Lua 5.1, 5.2, 5.3, 5.4, and
5.5** in pure Rust with zero non-build dependencies (cranelift is
the JIT codegen).

### Correctness

- **910 tests / 0 failures / 0 ignored**
  - 242 lib unit
  - 123 PUC official-suite files across 5 dialects (5.1 = 23,
    5.2 = 26, 5.3 = 27, 5.4 = 32, 5.5 = 15)
  - 40 end-to-end programs × 5 dialects byte-diff vs installed PUC
    binary
  - 64 method-JIT dialect-audit tests (`Value`-variant introspection)
  - 28 trace-JIT audit tests
  - 13 C API conformance tests
  - 10 sandbox embedding tests
  - 8 fast smoke tests
  - ~500 trace-JIT integration tests

### Performance

Master gate (`vs.X ≤ 0.50`, luna ≥ 2× the reference):

- **vs PUC 5.1-5.5: 35 / 35 cells PASS** across all 7 microbench
  workloads × 5 dialects
- **vs LuaJIT 2.1: 6 / 7 cells PASS**. `binary_trees_n10` lands at
  0.83× (luna 1.21× faster than LuaJIT 2.1, just shy of the 2× gate)
  — this is the design ceiling under luna's no-NaN-boxing + PUC
  bytecode-compat constraints.

See `docs/performance.md` for the full snapshot.

### Public surface (frozen for 1.x)

- Rust embedding API: `Vm`, `Value`, `LuaVersion`, the `Vm::open_*()`
  stdlib loaders, the native-function registration helpers
- Script-host sandbox pattern: see `examples/sandbox_demo.rs` and
  `tests/sandbox.rs`
- C ABI: `lua.h`-compatible subset under `src/capi.rs`, conformance
  locked by `tests/capi.rs`
- Bytecode binary compat: PUC-compiled `.luac` files load directly
  into luna for the corresponding dialect; luna's compiler emits
  matching format

### Major features

- Full dialect support — all 5 Lua versions in a single binary,
  per-`Vm` dialect selection
- Cranelift method-JIT for hot top-level chunks + cranelift
  trace-JIT for hot loop / recursive shapes
- PUC-faithful Lua semantics including: integer subtype (5.3+),
  bitwise operators (5.3+), `<const>` / `<close>` attributes (5.4+),
  `global` keyword + named varargs (5.5+), `goto` / labels (5.2+),
  full coroutine + metatable + weak-table + `__gc` finalizer
  support, generational GC pacing
- Sandbox-grade embedding: per-`Vm` instruction + memory budgets,
  bytecode-load gating, host native callbacks, no required global
  state

### Documentation

- `README.md` — overview + quick-start
- `docs/compatibility.md` — embedder compatibility surface
- `docs/performance.md` — perf snapshot
- `cargo doc --open` — full API reference

### Test environment

Tested on macOS 25.5 / aarch64 (M-series) with rustc 1.86+ and
cranelift 0.124. PUC binaries: Lua 5.1.5, 5.2.4, 5.3.6 built from
source; Lua 5.4.8, 5.5.0 + LuaJIT 2.1.1781602682 via brew.
