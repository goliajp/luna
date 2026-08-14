# Security Policy

## Supported versions

The latest minor of the current major receives security patches. The
last minor of the prior major receives security-only fixes for twelve
months after the major shipped. The patch cadence is opportunistic;
backports beyond this table are not guaranteed.

| Version | Supported |
|---|---|
| 3.x (latest minor) | ✅ |
| 2.20.x | ⚠️ Security only, until 2027-08-14 |
| < 2.20 | ❌ |
| 1.x | ❌ |

Upgrading from 2.x costs nothing: 3.0 made no breaking change — the
public surface is identical to 2.18.0 — so the supported version is a
version bump away. See [`CHANGELOG.md`](CHANGELOG.md).

## Reporting a vulnerability

luna is single-maintainer + does not accept external contributions.
The repository is public for transparency + dogfood; security
disclosure remains private.

**Do not open a public GitHub issue for security vulnerabilities.**

Email `admin@golia.jp` with:

1. A clear reproduction (Lua snippet + Rust embed code if applicable)
2. The luna version (`luna --version` or `cargo tree -p luna-jit`)
3. The platform (OS + arch + Rust toolchain version)
4. Your assessment of impact (sandbox escape / DoS / info leak / etc)
5. A 90-day disclosure window proposal (default: 90 days from report
   to public disclosure, accelerated if a fix lands sooner)

You can expect:

- Acknowledgement within 5 business days
- A triage decision (in-scope / not-in-scope) within 14 days
- For in-scope reports, a fix target date + CVE coordination
- Public disclosure via `CHANGELOG.md` + a GitHub Security Advisory
  on the agreed window

## Scope

In scope:

- Sandbox escape from `Vm::new` (no `open_os_io` / `open_debug` /
  `open_package` called) — any path that reaches `std::process` or
  `std::fs` from inside Lua
- Memory safety issues in the JIT path (`luna-jit`)
- Memory safety issues in the AOT runtime helpers (`luna-runtime-helpers`)
- Bytecode loader vulnerabilities (`Vm::allow_bytecode_loading`) when
  the loader is fed deliberately malformed `.luac`
- Cross-thread races under `feature = "send"` SendVm
- Userdata `__gc` finalizer ordering bugs that allow use-after-free

Out of scope:

- Lua VM semantic bugs that don't have a security impact (file these
  as regular issues, or run them through the dogfood report channel)
- Performance issues — see [`docs/performance.md`](docs/performance.md)
  for the measurement methodology and the per-release perf gate
- Issues in `cargo audit`-flagged transitive dependencies that don't
  reach a luna call path (file upstream)
- Compromise of the `crates.io` publishing pipeline (separately
  monitored)

## Defense-in-depth principles

luna's threat model + defense-in-depth contracts are documented in
[`docs/security.md`](docs/security.md). Key invariants:

- **0 unsafe at the embedder surface** — any `pub` item that requires
  the caller to write `unsafe` is a regression
- **luna-core 0 third-party deps** — the smallest crate ships with
  zero supply-chain surface; the contract is CI-enforced via
  `cargo deny check`
- **Opt-in OS facilities** — `io`, `os`, `debug`, `package` are
  closed by default; embedders must explicitly open them

## Acknowledgements

Researchers who disclose responsibly are credited, with their consent,
in the release notes for the fix and in a `THANKS.md` that will be
created at the first such disclosure — there has not been one yet, so
the file does not exist. For embargoed reports, credit lands at the
disclosure window.

---

*Last updated 2026-08-15. For the threat model + sandbox boundary
documentation, see [`docs/security.md`](docs/security.md).*
