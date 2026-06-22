//! luna-core — pure-Rust Lua runtime (interpreter only).
//!
//! Zero third-party dependencies. The JIT-equipped variant lives in
//! the [`luna`](https://docs.rs/luna) crate, which re-exports
//! everything here and additionally installs the Cranelift backend.
//!
//! Primary dialect: Lua 5.5 (tracks official upstream).
//! Compat modes: Lua 5.1 / 5.2 / 5.3 / 5.4.
//!
//! # Embedding contract (script-host sandbox)
//!
//! The minimal embedding flow:
//!
//! 1. `Vm::new_minimal(version)` — empty VM, no libraries loaded, no
//!    JIT installed (the trait slots default to a no-op backend in
//!    luna-core).
//! 2. `vm.open_base()` / `open_math()` / `open_string()` /
//!    `open_table()` / `open_coroutine()` — whitelist only the safe
//!    subset; skip `os` / `io` / `debug` / `package`.
//! 3. `vm.set_bytecode_loading(false)` when running untrusted scripts
//!    (see caveats below).
//! 4. `vm.set_instr_budget(Some(N))` + `vm.set_memory_cap(Some(M))`
//!    per request.
//! 5. `vm.load(src, name)` → `vm.call_value(closure, args)`.
//! 6. On error: `vm.error_text(&e)` + `vm.take_error_traceback()`.
//!
//! See `examples/embed_min.rs` for a runnable zero-dep walkthrough.
//!
//! ## Sandbox caveats
//!
//! - **JIT bypass of `instr_budget`** — the (optional) cranelift JIT
//!   compiles counted-for loops to native code that does not tick the
//!   budget. Sandbox embedders that link against `luna` (not
//!   `luna-core`) must call `vm.set_jit_enabled(false)` before
//!   running untrusted scripts.
//! - **Bytecode load surface** — `load()` defaults to mode `"bt"`
//!   which accepts precompiled chunks, bypassing the parser's
//!   depth/opcode limits. Sandbox embedders should call
//!   `vm.set_bytecode_loading(false)`.
//! - **`instr_budget` / `mem_cap` are fire-once** — both clear to
//!   `None` on first trip. Re-arm before each `call_value` if reusing
//!   the Vm across requests, or (recommended) create a fresh Vm per
//!   request for isolation.
//! - **`heap.bytes()` is approximate** — internal `Vec`/`Box`
//!   capacity overhead is not auto-tracked, so the cap is a lower
//!   bound. Size it with ~2× margin over the desired hard limit;
//!   bound the Vm's lifetime as a second line of defense.
//! - **`error()` may carry any Value** — Lua scripts can call
//!   `error({code=…, msg=…})` with a non-string payload. Use
//!   `vm.error_text(&e)` to normalize to a string, or inspect `e.0`
//!   directly when the host protocol expects structured errors.
//! - **Native panic = Vm-fatal** — Rust panics in `NativeFn`
//!   callbacks are caught and surfaced as `LuaError("native panic:
//!   …")`, but the Vm state may be inconsistent afterwards. Drop the
//!   Vm on any error whose text starts with `"native panic:"`.
//! - **5.1 / 5.2 lack `string.pack`/`unpack`/`packsize`** — these are
//!   5.3+ additions, gated by version. 5.1 hosts (script host) see
//!   them as `nil` on the `string` table.
//! - **5.1 numeric semantics** — Lua 5.1 has no integer subtype.
//!   Arithmetic and the `for` loop always produce `Value::Float`;
//!   hosts that route results back through Rust must accept either
//!   `Int` (5.3+) or `Float` (all versions).
//! - **`collectgarbage("count")` is a timing channel** — scripts can
//!   observe rough heap pressure changes. Acceptable for typical
//!   embedding; flag if the threat model includes side-channel
//!   probing.

pub mod compiler;
pub mod frontend;
pub mod jit;
pub mod numeric;
pub mod pattern;
pub mod runtime;
pub mod version;
pub mod vm;
