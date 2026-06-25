# Security

luna is an embedded Lua VM. The host program — not the Lua script —
owns the security boundary: every capability a script sees was
opted in by Rust code. This page documents what luna protects
against, what it doesn't, and the knobs the embedder controls.

For the `unsafe` Rust audit (separate concern: memory-safety of the
implementation itself), see [`unsafe-accounting.md`](unsafe-accounting.md).

---

## 1. Threat model

### In scope

luna's sandbox is designed to defend against the following when the
embedder follows the patterns in §2:

- **Script-driven OS access exfiltration.** An untrusted Lua chunk
  cannot reach the filesystem, environment, network, child processes,
  or the embedder's address space beyond what the host exposed.
- **Resource exhaustion (CPU / memory).** Per-`call_value` instruction
  budget and approximate-byte memory cap force a catchable Lua error
  before the host loses control.
- **Bytecode-loader escape.** Precompiled chunks bypass the parser's
  depth and opcode-shape limits; both luna's own dump format and PUC
  `.luac` loading are off by default in `Vm::sandbox(...)`.
- **Unsound `unsafe` at the embedder surface.** Every public
  `cargo doc`-visible API is safe Rust. The four `pub unsafe fn`
  remnants are `#[doc(hidden)]` (see
  [`unsafe-accounting.md`](unsafe-accounting.md) §5).
- **`__gc` finalizer misuse from Rust userdata.** Drop ordering for
  `LuaUserdata` finalizers is bounded by the GC sweep; see §6.

### Out of scope

- **Lua VM correctness bugs.** A logic bug in dispatch / GC / a stdlib
  function may produce wrong results or panic; it is not modeled as
  a sandbox escape. Report as a normal bug.
- **Side-channel attacks.** No constant-time guarantees on hash table
  ops, string interning, or numeric paths.
- **Supply-chain integrity.** luna-core has zero third-party deps;
  luna-jit / luna-aot pull `cranelift-*` / `object` / `clap` (see
  `cargo tree`). Verify your own lockfile.
- **Denial of service via legal-but-pathological scripts.** Budget
  caps bound CPU/memory per call but not, e.g., output volume to a
  host-exposed sink — the host owns that side.
- **`debug.*` library when opened.** `debug.setupvalue` /
  `debug.sethook` / `debug.getinfo` are explicitly *intended* to
  introspect the running program; opening `debug` to untrusted code
  is equivalent to handing it the Vm.
- **`os.execute` / `io.popen` when opened.** They shell out to
  `/bin/sh -c` (Unix) or `cmd /C` (Windows) on the host. Opening them
  to untrusted code is equivalent to handing it shell access.

---

## 2. Sandbox boundaries

The conservative default lives on `SandboxBuilder`
(`crates/luna-core/src/vm/sandbox.rs`), reached via
`Lua::sandbox(version)` (high-level) or `Vm::sandbox(version)`
(low-level). What the builder gives you:

| Surface | Default | Opt-in method |
|---|---|---|
| `base` stdlib (`print`, `pcall`, `type`, …) | **off** | `.open_base()` |
| `math` | **off** | `.open_math()` |
| `string` | **off** | `.open_string()` |
| `table` | **off** | `.open_table()` |
| `coroutine` | **off** | `.open_coroutine()` |
| `io` / `os` | **off, no builder knob** | `vm.open_os_io()` post-build (trusted hosts only) |
| `debug` | **off, no builder knob** | `vm.open_debug()` post-build (trusted hosts only) |
| `package` / `require` | **off, no builder knob** | `vm.open_package()` post-build (trusted hosts only) |
| `bit32` (5.2 only) | **off, no builder knob** | `vm.open_bit32()` post-build |
| Luna-dialect bytecode loading | **off** | `.allow_bytecode_loading()` |
| PUC `.luac` loading | **off** | `vm.set_puc_bytecode_loading(true)` post-build |
| Instruction budget | unbounded | `.with_instr_budget(n)` |
| Memory cap | unbounded | `.with_memory_cap(n)` |
| JIT backend | `NullJitBackend` (interp) on luna-core; `CraneliftBackend` on luna-jit | `Vm::install_jit_backend(...)` for custom |

The asymmetry — safe-subset stdlibs live on the builder, OS-touching
ones don't — is deliberate. A two-line typo at the call site (e.g. a
hypothetical `.open_io()` in a chain of `.open_*()` calls) would be
hard to spot in code review; forcing `vm.open_os_io()` on the
post-build handle makes the trust decision visible.

Example: untrusted scripting host.

```rust
use luna_jit::Lua;
use luna_jit::version::LuaVersion;

let mut lua = Lua::sandbox(LuaVersion::Lua54)
    .open_base()
    .open_math()
    .open_string()
    .open_table()
    .with_instr_budget(1_000_000)            // ~10 ms wall-clock
    .with_memory_cap(8 * 1024 * 1024)        // 8 MiB
    .build();

// `io`, `os`, `debug`, `package` are not in the global table.
// `load(bytecode)` rejects precompiled chunks.
let r: i64 = lua.eval("return 1 + 2").unwrap();
```

The instruction budget is consumed per dispatch turn and resets on
each new `call_value` / `eval` entry; the memory cap is fire-once
and disarms itself after raising, so re-arm before reusing the Vm
across requests. See `Vm::set_instr_budget` and `Vm::set_memory_cap`
rustdoc for the precise semantics.

---

## 3. OS facility gating

luna's `os_*` / `io_*` natives mirror PUC's surface but only register
when the embedder calls `vm.open_os_io()`. Once opened, every
function on the table is reachable from Lua. There is **no
finer-grained sub-gate** in v2.0 — opt in to all of `io.*` + `os.*`
or none.

Notable functions when `open_os_io()` is called:

- `io.open`, `io.lines`, `io.tmpfile`, `io.read`, `io.write`,
  `io.popen`, `io.input`, `io.output`, `io.close`, `io.flush`,
  `io.type` — full filesystem + process I/O.
- `os.time`, `os.clock`, `os.date`, `os.difftime`, `os.getenv`,
  `os.setlocale`, `os.tmpname`, `os.remove`, `os.rename`,
  `os.execute`, `os.exit` — clock, env, fs, shell.

For `debug.*` (`vm.open_debug()`): `debug.sethook`,
`debug.getinfo`, `debug.getupvalue`, `debug.setupvalue`,
`debug.traceback`, `debug.getlocal`, `debug.setlocal` — all are
introspection-into-the-running-program by design. Opening `debug`
to untrusted code defeats the rest of the sandbox.

For `package.*` (`vm.open_package()`): `require`, `package.searchers`,
`package.preload`, `package.loaded`. Dynamic linker loaders
(`package.loadlib`, C-side `package.cpath` resolution) are
deliberately stubbed (`nat_loadlib_stub` in
`crates/luna-core/src/vm/lib_os_io.rs`) — luna ships no host linker.
`require` still walks `package.path` for `.lua` sources, so
filesystem access is implied; pair `open_package()` with controlled
`package.path` if you mean to limit which `.lua` files load.

If you need a tighter gate than "all of `io+os`" (e.g. allow
`os.time` and `os.date` but not `io.open` or `os.execute`), the v2.0
path is: do not call `open_os_io()`, then add the specific natives
yourself with `vm.set_global("os", ...)` and `vm.table_of([...])`
patterns. A finer-grained partial-open helper is a candidate for
future versions but is not in v2.0.

---

## 4. `wasm32-wasip1` / `wasm32-wasip2` stubs

luna compiles cleanly to `wasm32-wasip1` (CI-gated in
`.github/workflows/ci.yml`). On that target, the natives that would
shell out are compiled as no-op stubs that match PUC's failure-return
shape so callers see the same error path:

- `io.popen(prog [, mode])` — validates args for parity with the
  native path, then returns `(nil, "popen not supported on this platform", -1)`
  (`crates/luna-core/src/vm/lib_os_io.rs:920`).
- `os.execute()` — no-arg form returns shell-unavailable
  (5.1: `0`; 5.2+: `false`). With a command arg, returns the PUC
  failure triple `(false, "exit", -1)` on 5.2+ or `-1` on 5.1
  (`crates/luna-core/src/vm/lib_os_io.rs:1760`).

Filesystem ops (`io.open`, `io.lines`, `os.remove`, `os.rename`,
`os.tmpname`) continue to use `std::fs`, which honors the wasi
preopened-directory permission model the wasi host supplied. luna
adds no further restriction — if the host preopened `/`, `io.open`
will see `/`.

This means **wasi alone is not a luna-level sandbox**. Use
`Vm::sandbox(...)` for that, and rely on the wasi host's preopens
to bound what the `open_os_io()` opt-in could reach.

---

## 5. Zero `unsafe` at the embedder surface

A4 (v1.1 charter): every type and function that `cargo doc` shows
to a downstream crate is safe Rust. The four remaining
`pub unsafe fn` (`Gc::<T>::as_mut`, `Value::as_closure_unchecked`,
`Value::as_int_unchecked`, `Value::pack`) are `#[doc(hidden)]` —
they exist for the JIT and capi paths, not for embedders.
See [`unsafe-accounting.md`](unsafe-accounting.md) §5 for the
exhaustive list.

For embedder authors auditing their own glue:

- Prefer `Lua` / `LuaRoot` / `LuaTable` / `LuaFunction` over
  `Vm` / `Gc<Table>` / `Gc<LuaClosure>` directly — the facade types
  hide the GC-handle ownership shape behind safe wrappers.
- Use `TableBuilder` (`vm.new_table().with(...).build()`) instead
  of `Gc::<Table>::as_mut`-style mutation.
- Use `vm.native_typed(|...|)` (auto-decode/encode) instead of the
  raw `fn(&mut Vm, u32, u32) -> Result<u32, LuaError>` shape.

If your embedder needs `unsafe` to call into luna, you are using a
deeper-than-necessary API; file an issue describing the use case so
the safe layer can be extended.

---

## 6. Userdata FFI safety

Rust types bridged via `LuaUserdata`
(`crates/luna-core/src/vm/userdata_trait.rs`) have two safety
contracts the embedder is responsible for:

### `trace`

If your `T: LuaUserdata` holds any `Gc<...>` field, you **must**
override `LuaUserdata::trace(&self, m: &mut UserdataMarker)` and
mark every embedded handle. The default `trace` is a no-op, suitable
only when `T` holds no GC pointers.

Inside `trace`:

- You may call `m.mark(gc_handle)` only.
- You may **not** call into the `Vm`, take locks, allocate, or
  perform I/O. The trace runs synchronously inside a GC pass; doing
  anything else risks re-entrant GC or deadlock.

A missing `trace` on a userdata holding a `Gc<Table>` is a
use-after-free waiting to happen: the GC will not see the embedded
table as reachable and will sweep it; the next access from Rust
through your stale `Gc` is undefined behavior.

This is the only userdata invariant that is unsound to break. The
`LuaUserdata` derive macro (`luna_jit_derive::LuaUserdata`) is the
recommended way to get `trace` right by construction.

### `__gc` finalizer

`LuaUserdata::add_methods` may register a `MetaMethod::Gc` handler.
PUC semantics apply:

- The finalizer fires during the GC sweep that collects the
  userdata, before the underlying Rust value is dropped.
- Order of `__gc` calls within a single sweep is unspecified;
  do not assume one userdata's `__gc` runs before another's.
- Errors raised from `__gc` are routed through the warn-state path
  on 5.4+ (matching PUC), and through the raise path on 5.1–5.3.
- Rust's `Drop for T` still runs after the `__gc` finalizer.
  Do not rely on `Drop` order across multiple userdata.

The finalizer-from-Lua path also goes through this gate — if the
embedder exposes `setmetatable(u, mt_with_gc)`-style flows to Lua
code, the same ordering caveats apply.

---

## 7. Reporting a vulnerability

luna does not yet have a published `SECURITY.md` with a coordinated
disclosure window (planned post-v2.0). Until then:

- Open a private security advisory on the GitHub repo
  (`Security` → `Report a vulnerability`).
- If GitHub's advisory flow is unavailable, email the repository
  owner (see the `goliajp/luna` GitHub profile) with `[luna
  security]` in the subject.
- Do not file public issues for security reports.

Please include: affected version (`luna-core` / `luna-jit` /
`luna-aot`), reproducer, and (if known) which sandbox knob was
expected to gate the behavior.

---

## See also

- [`embedding.md`](embedding.md) §3 — sandbox builder cookbook
- [`unsafe-accounting.md`](unsafe-accounting.md) — `unsafe` block
  audit and `pub unsafe fn` inventory
- [`compatibility.md`](compatibility.md) — per-dialect feature matrix
- [`threading.md`](threading.md) — `Vm: !Send` rationale and the
  forward `feature = "send"` roadmap
