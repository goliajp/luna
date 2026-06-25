# Security Policy

## Supported versions

Only the latest minor release on the `1.x` line receives security
patches. The patch-level cadence is opportunistic; backports to
older minor versions are not guaranteed.

| Version | Supported |
|---|---|
| 1.3.x | ✅ |
| 1.2.x | ❌ (collapsed into 1.3 per nodefer charter — never published) |
| 1.1.x | ⚠️ Best-effort backport on request |
| 1.0.x | ❌ |
| < 1.0 | ❌ |

When v2.0 ships, the support window will shift to "latest minor of
the current major + last minor of the prior major (security only,
12-month window)".

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
- Performance issues (use the v2.0 charter Track BM bench gate)
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

A `THANKS.md` file enumerates security researchers who have
responsibly disclosed issues (with consent). For embargo'd reports,
acknowledgement happens at the disclosure window.

---

*Last updated 2026-06-25. For the threat model + sandbox boundary
documentation, see [`docs/security.md`](docs/security.md).*
