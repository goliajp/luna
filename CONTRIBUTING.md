# Contributing

luna is a single-maintainer project. **External contributions are
not accepted.**

The repository is public for transparency, dogfood, and ecosystem
visibility — it is not open to PRs, design proposals, or feature
requests through the issue tracker.

## What this means in practice

- **PRs will be closed without review.** This is a maintenance
  policy, not a comment on the work.
- **Feature-request issues are not the right channel.** If you have
  a workload luna doesn't fit, fork it — the license permits this.
- **Bug reports are welcome but unowned.** If you file an issue, do
  not expect a fix timeline. Reproduce-able bugs get triaged
  opportunistically.
- **Security disclosures go to email, not the tracker.** See
  [`SECURITY.md`](SECURITY.md) for the disclosure flow.

## Why this policy

luna is the runtime for [`goliajp/kevy`](https://github.com/goliajp/kevy)
and a handful of other GOLIA K.K. projects. The single-maintainer
shape lets the project evolve to those workloads' needs without
the coordination overhead of multi-party review. v2.0 in particular
is a "no defer" sprint where every track lands or is permanently
out-of-scope — that cadence isn't compatible with community-PR
review windows.

## If you want to fork

The MIT/Apache-2.0 dual license is unconditional. Fork freely,
remove this notice, and run your own roadmap. The git history,
charter docs (in the repo's `.dev/` directory — gitignored, not
distributed in `cargo publish` tarballs), and CHANGELOG document
the design rationale enough that an independent fork has full
context.

## If you're here for the implementation notes

- [`docs/architecture.md`](docs/architecture.md) — workspace map +
  steel-cement-stone module classification
- [`docs/embedding.md`](docs/embedding.md) — `luna-core` / `luna-jit`
  library embed cookbook
- [`docs/aot.md`](docs/aot.md) — AOT single-binary deploy guide
- [`docs/deploy.md`](docs/deploy.md) — production deployment patterns
- [`docs/threading.md`](docs/threading.md) — cross-thread + tokio
- [`docs/security.md`](docs/security.md) — threat model
- [`docs/performance.md`](docs/performance.md) — perf methodology +
  bench reproduction
- [`docs/compatibility.md`](docs/compatibility.md) — per-dialect
  feature matrix (Lua 5.1 - 5.5 + MacroLua)
- [`docs/contributing-coverage.md`](docs/contributing-coverage.md) —
  test coverage reproduction
- [`docs/contributing-mem.md`](docs/contributing-mem.md) — memory
  baseline reproduction
- [`docs/contributing-disk.md`](docs/contributing-disk.md) — disk
  + binary size baseline reproduction
- [`docs/contributing-docs.md`](docs/contributing-docs.md) — docs CI
  gate reproduction (lychee + cargo doc + doctest)
- [`docs/migration-v1-to-v2.md`](docs/migration-v1-to-v2.md) —
  major-version upgrade checklist

## License

Dual-licensed under MIT or Apache-2.0, at your option. See
`LICENSE-MIT` and `LICENSE-APACHE` in the repo root.

---

*Last updated 2026-06-25.*
