# Release checklist

Reusable template for cutting a luna `vX.Y.Z` release. Maintainers
fill in the version, date, and headline summary then walk the gates
top-to-bottom. Sprint-specific track audits live in
`.dev/release-vX.Y.Z-checklist.md`; this file stays version-agnostic
so the procedure itself does not rot between releases.

Tested against `v1.1.0` (2026-06-23) and `v1.2.0` (2026-06-24); see
`.dev/release-v1.1.0-checklist.md` for the v1.1 historical record.

---

## 0. Pre-flight (sprint floor closed)

Before opening this checklist, confirm:

- [ ] All floor tracks of the sprint shipped (per `.dev/rfcs/vX.Y-charter.md`)
- [ ] `.dev/rfcs/vX.Y-plan-state.md` "当前 phase" reflects the closure
- [ ] CHANGELOG.md has a `## [X.Y.Z] — YYYY-MM-DD` section above
      `## [Unreleased]`, with an explicit **Deferred to vX.Y+1** list
      so honest scope is recorded at ship time (no silent defer)
- [ ] All `.dev/known-bugs/` items either fixed or explicitly noted in
      the release-notes draft

## 1. Version bump

```sh
# Edit workspace.package.version in Cargo.toml
# (luna-core + luna-jit both inherit via version.workspace = true)
vi Cargo.toml
```

## 2. Verification gates

All gates run locally before tagging. CI runs the subset marked **CI**
in `.github/workflows/ci.yml` on each push.

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
# luna-core must show exactly one crate in its dep tree (itself).
count=$(cargo tree -p luna-core --prefix none --no-default-features | grep -cE ' v[0-9]')
test "$count" -eq 1
```

### Unsafe drift (CI)

```sh
# First-party unsafe site count must not exceed the ceiling
# recorded in ci.yml's `unsafe-drift` job. Bump the ceiling
# explicitly if the sprint added justified unsafe.
grep -rE 'unsafe (\{|fn |impl |trait |extern )' \
  crates/luna-core/src crates/luna-jit/src | wc -l
```

### rustdoc clean (CI)

```sh
RUSTDOCFLAGS="-D warnings" cargo doc -p luna-core --no-deps
RUSTDOCFLAGS="-D warnings" cargo doc -p luna-jit --no-deps
```

### Supply chain (optional)

```sh
cargo install --locked cargo-deny 2>/dev/null
cargo deny check
```

### Embedder examples (smoke)

```sh
cargo run --example embed_min -p luna-core
cargo run --example embedding_quickstart -p luna-jit
cargo run --example userdata_demo -p luna-jit
cargo run --example userdata_vec3 -p luna-jit          # since v1.2
cargo run --example userdata_redis_stub -p luna-jit    # since v1.2
cargo run --example async_host -p luna-jit
cargo run --example sandbox_demo -p luna-jit
```

## 3. Tag sequence

```sh
# Confirm HEAD is the ship commit
git log -1 --oneline

# CHANGELOG: move [Unreleased] → [X.Y.Z] — YYYY-MM-DD
# (manual edit; preserve the Deferred-to-vX.Y+1 list verbatim)

# Annotated tag
git tag -a vX.Y.Z -m "luna vX.Y.Z — <headline>"

# Verify remote
git remote -v
# If empty: git remote add origin git@github.com:goliajp/luna.git

git push origin master   # or develop, depending on git-flow phase
git push origin vX.Y.Z
```

## 4. crates.io publish (staged: luna-core then luna-jit)

```sh
# 1. luna-core first — luna-jit's path-dep falls back to the
#    "= X.Y.Z" version pin on crates.io once published.
cargo publish -p luna-core

# 2. Wait for crates.io to index (5-30s typically).
cargo install --locked cargo-wait-for-publish 2>/dev/null
cargo wait-for-publish --package luna-core --version X.Y.Z

# 3. luna-jit (depends on luna-core; the index resolves the pin).
cargo publish -p luna-jit
```

## 5. GitHub release

```sh
# Draft release notes pulled from CHANGELOG.md [X.Y.Z] section.
gh release create vX.Y.Z \
  --title "luna vX.Y.Z" \
  --notes-file .dev/release-vX.Y.Z-notes.md
```

## 6. Post-tag follow-on (non-blocking)

- Announce: short release post / discussion linking README + docs
- Update GitHub repo description if headline changed
- Kick off v(X.Y+1) milestone: `.dev/rfcs/v(X.Y+1)-charter.md` +
  `.dev/rfcs/v(X.Y+1)-plan-state.md`
- Bump `.claude/CLAUDE.md` "当前状态" snapshot

## 7. Failure recovery

| Symptom | Recovery |
|---|---|
| `cargo publish` rejected (crate name taken / version exists) | Bump patch in `Cargo.toml`, re-tag, re-publish |
| crates.io indexing delay | `cargo wait-for-publish --package luna-core --version X.Y.Z`; if >5min, re-try luna-jit publish |
| Tag pushed but publish failed | Leave the tag in place, fix the publish issue, do not delete remote tag |
| CI lint job fails after push | Fix on a follow-on commit; no need to re-tag |
| Dependency surface changed (luna-core 0-dep contract broken) | Revert the offending commit and re-tag; do **not** widen the gate |

## 8. Per-version supplements

Each sprint generates one private supplement under `.dev/`:

- `.dev/release-vX.Y.Z-checklist.md` — track-by-track audit specific
  to that sprint's scope (not in public docs/ since it goes stale)
- `.dev/release-vX.Y.Z-notes.md` (optional) — release-notes draft
  used by `gh release create --notes-file`

See `.dev/release-v1.1.0-checklist.md` for the v1.1 archived shape.
