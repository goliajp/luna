# Contributing to luna docs

luna's documentation lives in three buckets and is protected by three
CI gates (`.github/workflows/docs.yml`). All three gates run on every
PR to `master` and `develop`; any one failing blocks merge.

## What's covered

| Bucket                          | Files                                            | Owner gate                |
| ------------------------------- | ------------------------------------------------ | ------------------------- |
| Rustdoc on public API           | `crates/*/src/**/*.rs` doc comments              | `cargo doc -D warnings`   |
| Doctests (rustdoc examples)     | ```` ```rust ```` blocks in doc comments         | `cargo test --doc`        |
| Markdown prose                  | `docs/**/*.md`, `crates/*/README.md`, root `*.md`| `lychee` link check       |

## Running the gates locally

Run all three before opening a PR. They take ~30 seconds combined on
a warm target dir.

```bash
# Gate 1 — rustdoc warnings = error.
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --workspace --all-features

# Gate 2 — doctests.
cargo test --doc --workspace --all-features

# Gate 3 — markdown link rot. Install lychee once via `brew install lychee`
# or `cargo install lychee`.
lychee --include-fragments --no-progress \
    'docs/**/*.md' 'crates/*/README.md' 'CHANGELOG.md' 'README.md'
```

If any of these complain, fix the underlying issue rather than silencing
the warning. `--include-fragments` catches in-doc anchor rot (e.g. a
markdown link `[X](#some-section)` after the section header is renamed).

## When a link must be allow-listed

Some links are knowingly broken — e.g. `https://docs.rs/luna-core/...`
URLs before a crates.io publish. Add a regex (one per line) to
`.lycheeignore` at the repo root, with a comment explaining why and
when the entry can be removed.

`.lycheeignore` matches the full URL via regex; lines starting with
`#` are comments.

## Coverage gaps

The three gates do NOT cover:

- **Prose accuracy.** They check syntax, not semantics. A doc that
  confidently misdescribes the behavior of a function still passes.
  Reviewers carry that load.
- **Out-of-tree references.** Links to GitHub issues, RFCs, the
  `kevy` sibling repo, or external blog posts pass through to live
  HTTP. lychee will surface 404s but won't catch a redirect-to-different-
  content (e.g. an issue closed as "won't fix" that the doc still
  cites as motivation).
- **Plugin marketplace docs.** This repo does not ship Claude Code
  plugins; if that changes, extend `docs.yml` to cover `plugins/**/*.md`.

## Adding new docs

When you add a new top-level doc under `docs/`, no workflow change is
needed — the lychee glob `docs/**/*.md` already covers it.

When you add a new crate, you must:

1. Add `crates/<new>/README.md` (or rely on the crate's `src/lib.rs`
   docstring being the rendered docs.rs landing page).
2. Confirm `cargo doc --no-deps --workspace --all-features` covers
   the new crate's `--features` matrix — if not, add `--features X` to
   the `RUSTDOCFLAGS` step in `.github/workflows/docs.yml`.
