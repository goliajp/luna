# Compatibility

The compatibility surface for embedders deciding whether luna fits their
host. Current as of **v3.0.0** (2026-08-14). For performance methodology
and measured baselines see [`performance.md`](performance.md).

---

## How compatibility is established

Not by inspection. A private corpus of **514 fixtures** runs against
stock PUC interpreters built from source — **5.1.5, 5.2.4, 5.3.6,
5.4.8, 5.5.1** — and every one must match byte for byte on stdout,
stderr and exit code, with zero skips, before a commit is green. CI
additionally asserts the 5.5 reference is exactly 5.5.1, so the basis
cannot drift when a runner image changes.

On top of that, **PUC's own test suite** runs end-to-end on all five
dialects with assert-count instrumentation, so a file that silently
stops early is caught rather than counted as a pass.

Roughly 35 real divergences were found and fixed this way over the v2.x
arc, each pinned by a fixture. That number is the argument for the
method: they were not visible any other way.

## Dialect support

luna implements **Lua 5.1, 5.2, 5.3, 5.4, 5.5** and **MacroLua** in a
single binary. The dialect is chosen per-`Vm` at construction
(`Vm::new(LuaVersion::Lua55)`); one process can host several Vms on
different dialects concurrently without interference.

Each dialect's frontend emits bytecode in PUC's binary format for that
dialect, so PUC-compiled `.luac` files load directly (see
[below](#loading-puc-luac-files)).

### Per-dialect feature matrix

Sourced from the capability predicates in
`crates/luna-core/src/version.rs`.

| Feature | 5.1 | 5.2 | 5.3 | 5.4 | 5.5 | MacroLua |
|---|:-:|:-:|:-:|:-:|:-:|:-:|
| **Numeric** | | | | | | |
| Integer subtype (`Int`) | ✗ | ✗ | ✓ | ✓ | ✓ | ✓ |
| `//` floor-divide | ✗ | ✗ | ✓ | ✓ | ✓ | ✓ |
| Bitwise `& \| ~ << >>` | ✗ | ✗ | ✓ | ✓ | ✓ | ✓ |
| Hex-float `0x1p4` | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Syntax** | | | | | | |
| `goto` / `::label::` | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Empty statement `;` | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `break` anywhere in a block | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Nested `[[...]]` long strings | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| **Strings** | | | | | | |
| `\xXX` / `\z` escapes | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ |
| `\u{XXXX}` unicode escape | ✗ | ✗ | ✓ | ✓ | ✓ | ✓ |
| **5.4+ attributes** | | | | | | |
| `local <const>` | ✗ | ✗ | ✗ | ✓ | ✓ | ✓ |
| `local <close>` | ✗ | ✗ | ✗ | ✓ | ✓ | ✓ |
| **5.5 exclusives** | | | | | | |
| `global` keyword | ✗ | ✗ | ✗ | ✗ | ✓ | ✗ |
| Named vararg `function f(...name)` | ✗ | ✗ | ✗ | ✗ | ✓ | ✗ |
| Collective attribute `local <const> a, b` | ✗ | ✗ | ✗ | ✗ | ✓ | ✗ |
| **MacroLua exclusives** | | | | | | |
| `@name(args)` compile-time macros | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ |

Two rows are "✗" because PUC 5.1 rejects them and luna reproduces that
faithfully: `break` must be the last statement of a block, and `[[`
inside a level-0 long string is an error ("nesting of `[[...]]` is
deprecated"). Both are 5.1 behaviour, not a luna limitation.

### MacroLua

`LuaVersion::MacroLua` sits between `Lua54` and `Lua55` in the enum, so
it inherits every 5.4-and-earlier capability predicate for free while
staying below the 5.5 gates. Its base semantics are 5.4; on top it adds
a parse-time expansion pass triggered by the `@` sigil.

The host registers macros in a per-`Vm` `MacroRegistry`; the language
itself provides the quoting forms — `@name(args)`, the brace-delimited
`@name{ body }`, `@quote{ ... }` to capture a body as a token, and
`@unquote(name)` to splice one back inside another expansion.

Implementation: `crates/luna-core/src/frontend/macro_expander.rs`.
Worked examples: `crates/luna-core/tests/macro_lua.rs` and
`cargo run --example macro_lua_demo -p luna-jit`.

There is no upstream canonical MacroLua spec; LuaMacro (Steve Donovan)
served as the spec proxy.

## Standard library coverage

Per-dialect stdlib lives in `crates/luna-core/src/vm/builtins.rs` and
`crates/luna-core/src/vm/lib_*.rs`. Each library is opened explicitly —
nothing is open until the host says so.

| Library | Opener | Coverage |
|---|---|---|
| `base` (assert, print, type, …) | `open_base` | full |
| `math` | `open_math` | full |
| `string` (incl. pattern matching) | `open_string` | full |
| `table` | `open_table` | full |
| `coroutine` | `open_coroutine` | full |
| `utf8` (5.3+) | `open_utf8` | full |
| `bit32` (5.2) | `open_bit32` | full |
| `io` + `os` | `open_os_io` | full — one opener covers both |
| `package` / `require` | `open_package` | full |
| `debug` | `open_debug` | partial; sandbox-hostile, opt-in |
| everything above | `open_all_libs` | convenience; not for untrusted code |

`io` and `os` share a single opener because they share a threat model:
either the host is giving the script filesystem and process access or it
is not.

`cargo run --example sandbox_demo -p luna-jit` shows the curated setup
for running untrusted code — `base + math + string + table + coroutine`,
bytecode loading off, with instruction and memory budgets.

## C API surface

`luna-jit` builds a `cdylib` / `staticlib` exposing a `lua.h`-compatible
subset from `crates/luna-jit/src/capi.rs`, so an existing C host can
link against it as a drop-in for the covered surface.

Covered — `crates/luna-jit/tests/capi.rs` is the conformance suite
(13 tests):

- `lua_State` lifecycle: `luaL_newstate`, `luaL_openlibs`, `lua_close`
- pushes: `lua_pushnil`, `lua_pushboolean`, `lua_pushinteger`,
  `lua_pushnumber`, `lua_pushstring`, `lua_pushlstring`,
  `lua_pushcfunction`
- reads: `lua_isnumber`, `lua_tointeger`, `lua_tonumber`,
  `lua_tostring`, `lua_type`, `lua_typename`
- stack: `lua_settop`, `lua_pop`, `lua_gettop`, `lua_pushvalue`
- tables: `lua_newtable`, `lua_settable`, `lua_gettable`,
  `lua_setfield`, `lua_getfield`, `lua_rawget`, `lua_rawset`
- calls: `lua_call`, `lua_pcall`
- load: `luaL_loadstring`, `luaL_loadbuffer`, `luaL_dostring`

Not covered — use the Rust API:

- userdata / lightuserdata / `lua_newuserdata`
- continuations (`lua_callk`, `lua_pcallk`)
- coroutines through the C API (`lua_resume`, `lua_yield`)
- debug hooks
- `luaopen_<lib>` C-symbol shims for individual libraries

The Rust `Vm` API is the primary embedding surface and is considerably
richer than the C one — see [`embedding.md`](embedding.md).

## Bytecode

### luna's own dumps

luna emits per-dialect bytecode in PUC's binary format — same header,
same instruction encoding, same constant-pool layout — so a chunk dumped
by luna loads in PUC and vice versa, within that dialect's instruction
set.

Loading is **off by default** (`Vm::set_bytecode_loading(true)` to
enable). Crafted bytecode can bypass checks the compiler enforces, so
a host running untrusted input should leave it closed.

### Loading PUC `.luac` files

Opt in with `Vm::set_puc_bytecode_loading(true)`, also **off by
default**. The translator decodes a PUC chunk and re-encodes its body
into luna's 65-op set; the resulting Proto then runs on luna's
interpreter and JIT like any other. This is a strictly larger trust
surface than luna's own loader — an embedder taking untrusted chunks
should keep both gates shut.

Every dialect loads. What each translator still refuses is narrow, and
is a rejection with a diagnostic rather than a misinterpretation:

| Dialect | Refuses |
|---|---|
| 5.1 | `GETGLOBAL` / `SETGLOBAL` with `Bx > 255` (the `EXTRAARG` form); unknown opcodes |
| 5.2 | integral `lua_Number` builds; `CONCAT` with `A != B`; `SETTABUP` with a register key; forward-jumping `TFORLOOP`; unknown opcodes |
| 5.3 | `TFORLOOP.A < 2` (no luna `iter_base` equivalent); non-zero format byte |
| 5.4 | nothing functional — only malformed chunks, and lowerings that would need a temp register above 255 |
| 5.5 | nothing functional — only malformed chunks and unknown opcodes |

The v2.1 opcode-shape work collapsed the twenty-four gaps an earlier
audit had listed, which is why this table is much shorter than it once
was. Generic `for`, RK operands, `LOADBOOL true+skip` and the immediate
arithmetic forms are all handled now.

Per-dialect translators: `crates/luna-core/src/vm/dump/puc/puc_5{1..5}.rs`.
Their module headers document the encoding differences in detail.

## Known correctness gaps

None open. The `.dev/known-bugs/` open directory is empty as of v3.0.0,
and three classes of use-after-free that the arc started with are fixed
— including one that a previous release had judged impossible to
reproduce and which turned out to be two all-platform GC bugs.

Files excluded from the official-suite gate are listed in the `excluded`
arrays of `crates/luna-core/tests/official_run.rs`, each with an inline
rationale. They are scope choices, not gaps — for instance `gc.lua`,
`gengc.lua` and `tracegc.lua` make allocator-timing assumptions that do
not hold for a different GC, and 5.5's `files.lua` wants a real
`/dev/full`, which exists on Linux and not on macOS.

## CLI

| Flag | Behaviour |
|---|---|
| `--lua=5.X` | Dialect: 5.1 / 5.2 / 5.3 / 5.4 / 5.5 (default 5.5) |
| `--sandbox` | Open base/math/string/table/coroutine only; reject bytecode loading |
| `--budget=N` | Cap dispatched instructions before raising |
| `--no-jit` | Install `NullJitBackend` — interpreter only |
| `--profile` | Print trace-JIT counters when the script finishes |
| `-e "<code>"` | Run inline code instead of a file |
| `-` | Read source from stdin |
| *(no args)* | Interactive REPL |

The REPL evaluates each line first as an expression (prefixed with
`return`), then retries it as a statement on syntax error, so both
expressions and assignments work. It has multi-line continuation and
history at `~/.luna_history`, honours `--lua=X`, and exits on Ctrl-D.
Tab completion and syntax highlighting are behind the
`repl-line-editor` feature.

## Quick verification

```sh
# Whole suite
cargo test --release --workspace

# One PUC test file, on a chosen dialect
cargo run --release -p luna-core --example runone -- \
  --lua=5.5 crates/luna-core/tests/official/lua-5.5.1-tests/calls.lua

# Differential corpus vs stock PUC (needs the reference binaries)
cargo test --release -p luna-core --test diff_puc

# Sandbox walkthrough
cargo run --release --example sandbox_demo -p luna-jit

# Microbench vs PUC and LuaJIT (both must be on PATH)
cargo bench --bench cross_dialect
```
