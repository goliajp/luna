# Release checklist

Reusable template for cutting a luna `vX.Y.Z` release. Fill in the
version, date and headline, then walk the gates top to bottom.
Sprint-specific track audits live in `.dev/release-vX.Y.Z-checklist.md`;
this file stays version-agnostic so the procedure does not rot between
releases.

Walked for every release through `v3.0.0` (2026-08-14).

Two things this checklist exists to prevent, both of which have actually
happened: a release that ships to crates.io while the website still
advertises the previous version, and a "green" gate that was never
running. Neither is caught by tests.

---

## 0. Pre-flight

- [ ] All floor tracks of the sprint shipped (per `.dev/rfcs/vX.Y-charter.md`)
- [ ] `.dev/rfcs/vX.Y-plan-state.md` "当前 phase" reflects the closure
- [ ] CHANGELOG.md has a `## [X.Y.Z] — YYYY-MM-DD` section above
      `## [Unreleased]`, with an explicit **Deferred to vX.Y+1** list —
      honest scope recorded at ship time, no silent defer
- [ ] All `.dev/known-bugs/` items either fixed or named in the notes
- [ ] **Run `.dev/rfcs/monthly-drift-sweep.md`** — it is due before any
      release, and §8 of it is release-specific. Every line in that file
      is something that was once green while broken.

## 1. Version bump

```sh
vi Cargo.toml          # workspace.package.version — all crates inherit
```

Then the places that do not inherit and will not fail a build if
forgotten:

- [ ] `web/` — the version string in the `.eyebrow` of all six pages.
      `web/deploy.sh` refuses to ship a tree where the six disagree,
      which is what a half-finished bump looks like.
- [ ] `README.md` — the **Status** section. The `luna-jit = "3"` install
      snippets are major-only and stay put across a minor.
- [ ] `SECURITY.md` — only on a major: the supported-version window.

`PERF_REF` in `ci.yml` is bumped *after* the tag exists — see §7.

## 2. Verification gates

All gates run locally before tagging. CI runs the subset marked **CI**
in `.github/workflows/`.

### Build (CI)

```sh
cargo build --workspace --all-targets
cargo build --workspace --release
cargo build -p luna-core --target wasm32-unknown-unknown
cargo build -p luna-core --target wasm32-unknown-unknown --release
```

### Tests (CI)

```sh
cargo test --workspace --lib
cargo test --workspace --release
cargo test --doc --workspace
```

### Lint / format (CI)

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

### 0-dep contract (CI)

```sh
# luna-core must show exactly one crate in its dep tree: itself.
count=$(cargo tree -p luna-core --prefix none --no-default-features | grep -cE ' v[0-9]')
test "$count" -eq 1
```

### Unsafe drift (CI)

```sh
# Must not exceed the ceiling recorded in ci.yml's `unsafe-drift` job.
# Raise the ceiling explicitly if the sprint added justified unsafe.
grep -rE 'unsafe (\{|fn |impl |trait |extern )' \
  crates/luna-core/src crates/luna-jit/src | wc -l
```

### rustdoc clean (CI)

```sh
RUSTDOCFLAGS="-D warnings" cargo doc -p luna-core --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p luna-jit --no-deps
```

### Public-surface audit

- [ ] A **minor** bump must show an empty removal list. A non-empty one
      is a semver violation and either the change reverts or the bump
      becomes major. This audit exists because 2.14.0 shipped a changed
      signature in a minor release — see
      [`migration-v1-to-v2.md`](migration-v1-to-v2.md).

Method and caveats in `.dev/rfcs/v2.17-api-stability-audit.md`;
append the result to `.dev/rfcs/v3.0-acceptance-evidence.md`.

### The README compiles

```sh
cargo run -p luna-jit --example readme_check
```

Both snippets in `README.md` live in that example. If the README drifts
from the API, this stops compiling.

### Supply chain

```sh
cargo deny check      # all features, not just default
```

### Embedder examples (smoke)

```sh
cargo run --example embed_min -p luna-core
cargo run --example embedding_quickstart -p luna-jit
cargo run --example userdata_demo -p luna-jit
cargo run --example userdata_vec3 -p luna-jit
cargo run --example userdata_redis_stub -p luna-jit
cargo run --example async_host -p luna-jit
cargo run --example sandbox_demo -p luna-jit
cargo run --example macro_lua_demo -p luna-jit
```

## 3. Tag sequence

```sh
git log -1 --oneline                       # confirm HEAD is the ship commit
git tag -a vX.Y.Z -m "luna vX.Y.Z — <headline>"
git push origin master
git push origin vX.Y.Z
```

Day-to-day work is on `develop`; `master` is the release branch
(git-flow). Merge before tagging.

## 4. crates.io publish

Seven crates, in dependency order. Each waits for the index before the
next one resolves against it.

```sh
for c in luna-core luna-jit-derive luna-jit-helpers luna-jit-llvm \
         luna-jit luna-runtime-helpers luna-aot; do
  cargo publish -p "$c"
  cargo wait-for-publish --package "$c" --version X.Y.Z
done
```

`cargo install --locked cargo-wait-for-publish` if it is missing.
`luna-tools` is `publish = true` but has never been released; it is not
part of the chain.

**Publishing cannot be undone.** A yanked version stays resolvable for
anyone with a lockfile.

## 5. GitHub release

```sh
gh release create vX.Y.Z \
  --title "luna vX.Y.Z" \
  --notes-file .dev/release-vX.Y.Z-notes.md
```

Notes are drawn from the CHANGELOG `[X.Y.Z]` section. If the version
number itself will be read as a claim — a major bump that breaks
nothing, say — answer that in the first paragraph, because the number is
seen before the prose.

## 6. Website

**This is part of the release, not after it.** The site is the first
thing a visitor sees and it does not update itself; luna's went four
months and three releases advertising a stale version because no step
here said to deploy it.

```sh
cd web
python3 paint.py && python3 punct.py    # if any page content changed
./deploy.sh --check                     # verify without uploading
./deploy.sh                             # verify, upload, confirm the origin
```

`deploy.sh` refuses a tree whose six pages disagree on the version,
ships the stylesheet under a content hash so a returning visitor cannot
be served the old one from cache, and confirms afterwards that the
origin serves the version in the tree. It never runs `caddy deploy` —
that regenerates the whole Caddyfile from CaddyStore and drops any site
not in the store.

- [ ] Site serves the new version at <https://luna.golia.jp>
- [ ] Content still matches reality — the dialect matrix, the fixture
      count, and the crate list are all claims that can go stale

## 7. Post-tag

- [ ] **Bump `PERF_REF` in `.github/workflows/ci.yml`** to the release
      commit. The perf gate compares HEAD against this reference on the
      same runner; leaving it behind silently widens the window a
      regression can hide in.
- [ ] Update `.claude/CLAUDE.md` "当前状态"
- [ ] Update the GitHub repo description if the headline changed
- [ ] Open the next milestone: `.dev/rfcs/v(X.Y+1)-charter.md` +
      `.dev/rfcs/v(X.Y+1)-plan-state.md`
- [ ] Announce, linking README and docs

## 8. Failure recovery

| Symptom | Recovery |
|---|---|
| `cargo publish` rejected (version exists) | Bump patch in `Cargo.toml`, re-tag, re-publish |
| crates.io indexing delay | `cargo wait-for-publish`; if > 5 min, retry the next crate |
| Tag pushed but publish failed | Leave the tag, fix the publish, do **not** delete the remote tag |
| CI lint fails after push | Fix on a follow-on commit; no re-tag needed |
| 0-dep contract broken | Revert the offending commit and re-tag; do **not** widen the gate |
| Site deployed with wrong content | Fix and re-run `deploy.sh`; it is idempotent and mirrors with `--delete` |
| Breaking change found after a minor shipped | Do not add a shim reflexively. Record it in the CHANGELOG with a migration table, and say plainly that the minor was a semver violation — see the 2.17.0 entry |

## 9. Per-version supplements

Each sprint generates one private supplement under `.dev/`:

- `.dev/release-vX.Y.Z-checklist.md` — track-by-track audit for that
  sprint's scope (kept out of `docs/` since it goes stale)
- `.dev/release-vX.Y.Z-notes.md` — the notes draft used above
