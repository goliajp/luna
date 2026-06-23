# Compatibility

Compatibility surface for embedders deciding whether luna fits their
host. Snapshot at v1.0.0 (2026-06-23). For perf numbers see
[`performance.md`](performance.md).

---

## Dialect support

luna implements **Lua 5.1, 5.2, 5.3, 5.4, and 5.5** in a single Rust
binary. The dialect is selected per-`Vm` at construction
(`Vm::new(LuaVersion::Lua55)`); a single process can host multiple
Vms running different dialects concurrently without interference.

Each dialect's frontend (parser + compiler) emits per-dialect
bytecode matching PUC's compiler binary format, so PUC-compiled
`.luac` files for the corresponding dialect load directly into
luna.

### Per-dialect feature matrix

Sourced from `src/version.rs`'s capability predicates:

| Feature | 5.1 | 5.2 | 5.3 | 5.4 | 5.5 |
|---|:-:|:-:|:-:|:-:|:-:|
| **Numeric** | | | | | |
| Integer subtype (`Int`) | ✗ | ✗ | ✓ | ✓ | ✓ |
| `//` floor-divide | ✗ | ✗ | ✓ | ✓ | ✓ |
| Bitwise `& \| ~ << >>` | ✗ | ✗ | ✓ | ✓ | ✓ |
| Hex-float `0x1p4` | ✗ | ✓ | ✓ | ✓ | ✓ |
| **Syntax** | | | | | |
| `goto` / `::label::` | ✗ | ✓ | ✓ | ✓ | ✓ |
| Empty statement `;` | ✗ | ✓ | ✓ | ✓ | ✓ |
| `break` anywhere in block | ✗ | ✓ | ✓ | ✓ | ✓ |
| Nested `[[...]]` long strings | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Strings** | | | | | |
| `\xXX` / `\z` escapes | ✗ | ✓ | ✓ | ✓ | ✓ |
| `\u{XXXX}` unicode escape | ✗ | ✗ | ✓ | ✓ | ✓ |
| **5.4+ attributes** | | | | | |
| `local <const>` | ✗ | ✗ | ✗ | ✓ | ✓ |
| `local <close>` | ✗ | ✗ | ✗ | ✓ | ✓ |
| **5.5+ exclusives** | | | | | |
| `global` keyword | ✗ | ✗ | ✗ | ✗ | ✓ |
| Named vararg `function f(...name)` | ✗ | ✗ | ✗ | ✗ | ✓ |
| Collective attribute `local <const> a, b` | ✗ | ✗ | ✗ | ✗ | ✓ |

## Standard library coverage

Per-dialect stdlib functions present in `src/vm/builtins.rs` +
`src/vm/lib_*.rs`. The whitelisted subset suitable for sandboxed
embedding is exposed via `Vm::open_*()` methods:

| Library | `open_*` method | Coverage |
|---|---|---|
| `base` (assert, print, type, etc.) | `open_base` | full |
| `math` | `open_math` | full |
| `string` (incl. pattern matching) | `open_string` | full |
| `table` (concat, sort, insert, remove, unpack) | `open_table` | full |
| `coroutine` | `open_coroutine` | full |
| `io` | `open_io` | full (host-controlled) |
| `os` | `open_os` | full (host-controlled) |
| `debug` | not exposed by default | partial — host can opt-in |
| `package` / `require` | `open_package` | full (host-controlled) |
| `utf8` (5.3+) | `open_utf8` | full |

`examples/sandbox_demo.rs` shows the curated stdlib setup for a
script host running untrusted code: `base + math + string + table
+ coroutine` with bytecode-load disabled and instruction + memory
budgets gating each call.

## C API surface

luna ships a `cdylib` / `staticlib` exposing a `lua.h`-compatible C
ABI subset under `src/capi.rs`. Existing PUC consumers (e.g. C
modules linking against `liblua.so`) can link against luna's
`libluna.{so,dylib}` as a drop-in replacement for the API surface
covered.

Covered (`tests/capi.rs` is the conformance suite, 13 tests):

- `lua_State` lifecycle: `luaL_newstate`, `luaL_openlibs`, `lua_close`
- value pushes: `lua_pushnil`, `lua_pushboolean`, `lua_pushinteger`,
  `lua_pushnumber`, `lua_pushstring`, `lua_pushlstring`,
  `lua_pushcfunction`
- value reads: `lua_isnumber`, `lua_tointeger`, `lua_tonumber`,
  `lua_tostring`, `lua_type`, `lua_typename`
- stack manipulation: `lua_settop`, `lua_pop`, `lua_gettop`,
  `lua_pushvalue`
- table API: `lua_newtable`, `lua_settable`, `lua_gettable`,
  `lua_setfield`, `lua_getfield`, `lua_rawget`, `lua_rawset`
- call API: `lua_call`, `lua_pcall`
- script load: `luaL_loadstring`, `luaL_loadbuffer`, `luaL_dostring`

Not yet covered (use the Rust API or contribute):

- userdata / lightuserdata / `lua_newuserdata`
- continuations (`lua_callk`, `lua_pcallk`)
- coroutines via C API (`lua_resume`, `lua_yield`)
- debug hooks
- `luaopen_<lib>` C-symbol shims for individual stdlib loading

## Embedding compatibility

`tests/sandbox.rs` (10 tests) covers the script-host sandbox shape:
per-request short-lived `Vm`s with curated stdlib, instruction +
memory budgets, host-registered native callbacks. See
`examples/sandbox_demo.rs` for the canonical pattern.

The Rust `Vm` API is the primary embedding surface — richer than
the C API and with full async-safe ownership. `cargo doc --open`
renders the full embedding contract.

## Bytecode compatibility

luna loads PUC's compiled bytecode (`.luac` files) for every
supported dialect. The compiler binary format matches PUC's exactly
— same headers, same instruction encoding, same constant pool
layout.

luna's own compiler emits the same format; bytecode dumped from
luna loads in PUC and vice versa (within the dialect's instruction
set).

Bytecode loading is **off by default in sandbox mode**
(`set_bytecode_loading(false)`) because maliciously crafted
bytecode can bypass type checks that the compiler enforces.

## Known correctness gaps

At v1.0.0: **none**.

PUC files NOT in `tests/official_run.rs::SUITES`'s expected-pass
lists are gated by the suite's `excluded` arrays with inline
rationale. Typical exclusions:

- `debug.lua`-derived tests — luna's `debug` library is sandbox-
  hostile and not exposed by default
- `files.lua` — requires OS-specific `/tmp`-style filesystem
  assumptions
- `gengc.lua` (5.5) — allocator-specific timing assumptions

These are scope choices for the test gate, not correctness gaps.

## v1.1 luna-specific extensions

These are luna API additions on top of the PUC dialect support
above. None affect PUC bytecode compatibility or change Lua-side
semantics; they're embedder-facing only.

| Track | Item | Added |
|---|---|---|
| A1 | Workspace split (`luna-core` 0-dep / `luna` with JIT) | crate boundary |
| A2 | `JitState` sidecar on Vm | layout |
| A7 | `Vm: !Send` compile-time enforcement + `docs/threading.md` | doc + doctest |
| B1 | `Vm::sandbox(version).build()` builder | API |
| B2 | `vm.eval(src)` / `vm.eval_chunk(src, name)` returning `Result<Vec<Value>, LuaError>` | API |
| B3 | `vm.new_table().with(k, v).build()` + `vm.table_of([...])` | API |
| B4 | `IntoValue` trait + generic `vm.set_global<V: IntoValue>` | API |
| B5 | `vm.native_typed` + `FromLuaArgs` / `IntoLuaReturn` / `FromLuaValue` (arities 0-6) | API |
| B6 | `LuaErrorKind` enum + `Display`/`Error` impls + `vm.error_kind` / `vm.error_source` | API |
| B7 | `vm.intern_str` / `Value::try_as_str` / `Value::as_bytes` | API |
| B8 | `vm.create_userdata::<T>` / `set_userdata` / `userdata_borrow` for `T: 'static` host types | API |
| B9 | `vm.create_coroutine` / `vm.resume_coroutine` | API |
| B10 | `vm.eval_async` (Stage 1 shipped; Stages 2-3 in flight) | API |
| B11 | `vm.set_rust_debug_hook` + `RustHookEvent` | API |
| B12 | `luna::Lua` newtype facade (`Lua::new` / `Lua::sandbox` / `create_function` / etc.) | API |

`luna-core` keeps the same dialect surface as `luna`; the only
difference is which JIT backend installs by default
(`NullJitBackend` vs `CraneliftBackend`). PUC `.luac` bytecode
binary compat is identical between the two — no JIT means no
codegen, but bytecode load/save behavior is interp-side.

## CLI options (luna binary)

| Flag | Behavior |
|---|---|
| `--lua=5.X` | Select dialect (5.1 / 5.2 / 5.3 / 5.4 / 5.5; default 5.5) |
| `--sandbox` | SandboxBuilder shape: open base/math/string/table/coroutine only; reject bytecode loading |
| `--budget=N` | `set_instr_budget(Some(N))` before running |
| `--no-jit` | Install `NullJitBackend` (interpreter-only run) |
| `--profile` | After the script finishes, print trace-JIT counters from the `JitState` sidecar |
| `-e "<code>"` | Run inline code instead of a file |
| `-` | Read source from stdin |
| (no args) | Drop into interactive REPL — see "REPL behavior" below |

### REPL behavior

luna's no-arg path drops into an interactive prompt. Each line is
first evaluated as an expression (prepended with `return`); on
syntax error it's retried as a statement (so assignments and
function definitions work too). Ctrl-D / EOF exits cleanly.

The REPL respects `--lua=X` for dialect selection. v1.1 ships
single-line; multi-line continuation and command history land in
v1.2.

## Quick verification

```sh
# Run the entire test suite (~30s)
cargo test --release

# Run perf microbench (needs PUC + LuaJIT on PATH)
cargo bench --bench cross_dialect

# Run a single PUC test file
cargo run --release --example runone -- --lua=5.5 tests/official/lua-5.5.0-tests/calls.lua

# Sandbox embedding walkthrough
cargo run --release --example sandbox_demo
```
