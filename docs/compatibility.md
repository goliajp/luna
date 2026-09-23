# Compatibility

The compatibility surface for embedders deciding whether luna fits their
host. Current as of **v3.0.0** (2026-08-14). For performance methodology
and measured baselines see [`performance.md`](performance.md).

---

## How compatibility is established

Not by inspection. A private corpus of **514 fixtures** runs against
stock PUC interpreters built from source — **5.1.5, 5.2.4, 5.3.6,
5.4.9, 5.5.1** — and every one must match byte for byte on stdout,
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

PUC-compiled `.luac` files of any dialect load through the per-dialect
translators (see [below](#loading-puc-luac-files)); luna's own
`string.dump` uses a luna-specific body format, not PUC's.

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

Numbers are rendered the way PUC renders them, including the one place
where PUC's output depends on the platform: a NaN is spelled by the C
library's `printf`, so luna follows the platform it runs on — `nan` on
macOS; `-nan` for a negative NaN (which is what 0/0 gives on x86) and
`+nan` / ` nan` under those `string.format` flags on Linux; `-nan(ind)`
for that default NaN on Windows.

`io` and `os` share a single opener because they share a threat model:
either the host is giving the script filesystem and process access or it
is not.

`cargo run --example sandbox_demo -p luna-jit` shows the curated setup
for running untrusted code — `base + math + string + table + coroutine`,
bytecode loading off, with instruction and memory budgets.

## C API surface

`luna-jit` builds a `cdylib` / `staticlib` exposing a subset of the
`lua.h` functions from `crates/luna-jit/src/capi.rs`. It is **not** a
drop-in replacement for PUC's library: a host compiled against PUC's own
`lua.h` does not link, because several of the names below are macros in
those headers that expand to functions luna does not export
(`lua_tostring` → `lua_tolstring`, `lua_pcall` → `lua_pcallk` in 5.2+,
`lua_pushcfunction` → `lua_pushcclosure`, `lua_tointeger` →
`lua_tointegerx`). Call the covered functions by these names directly.

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

`string.dump` and `Vm::dump` emit luna's own binary format: the running
dialect's PUC header, then a `"\x00LunaV1\x00"` sentinel and a body in
luna's 65-op instruction set. It is **not** PUC's body format — a luna
dump loads back into luna, not into PUC, and a PUC `.luac` is read by the
translators below, not by this loader. luna does not emit PUC-format
bytecode; a `string.dump` that PUC could load is a separate feature the
owner has not committed to.

Loading a luna dump is gated by `Vm::set_bytecode_loading(false)` (on by
default; the `sandbox` builder turns it off). Crafted bytecode bypasses
checks the compiler enforces, so a host taking untrusted input should
close it.

### Loading PUC `.luac` files

Opt in with `Vm::set_puc_bytecode_loading(true)`, **off by default**. The
translator decodes a PUC chunk of any of the five dialects and re-encodes
its body into luna's 65-op set; the resulting Proto then runs on luna's
interpreter and JIT like any other. This is a strictly larger trust
surface than luna's own loader — an embedder taking untrusted chunks
should keep both gates shut, and read the safety note below.

Every dialect loads. A translator's refusals are the chunk shapes luna's
instruction set cannot hold, each a rejection with a diagnostic rather
than a misinterpretation:

| Dialect | Refuses |
|---|---|
| 5.1 | integral `lua_Number` builds; big-endian chunks; a mapped register, constant index or jump distance past luna's field width; unknown opcodes |
| 5.2 | integral `lua_Number` builds; the same field-width limits; unknown opcodes |
| 5.3 | non-zero format byte; a `lua_Integer` / `lua_Number` representation other than little-endian 64-bit; the same field-width limits; unknown opcodes |
| 5.4 | the same field-width limits; unknown opcodes |
| 5.5 | the same field-width limits; unknown opcodes |

The whole diff_puc corpus, compiled by each dialect's stock `luac`, loads
and runs — matching PUC byte for byte in stdout, and in the error channel
for the `_err` fixtures — under `diff_puc.rs::diff_puc_bytecode`, on the
interpreter and (in `luna-jit`) under the JIT.

The translators do not verify a chunk PUC's own compiler could not have
produced. luna's interpreter reads registers and constants without bounds
checks, trusting its compiler; the translators uphold that trust for
every real `.luac`, but a hand-corrupted register field that stays inside
luna's 8-bit range yet exceeds the frame's `max_stack` is read past the
stack — a memory-safety fault on crafted input. Closing that needs a
bytecode verifier (every register against `max_stack`, every constant
index, every jump target), which luna, like PUC after 5.1, does not ship.
Keep `set_puc_bytecode_loading` off for untrusted chunks.

Per-dialect translators: `crates/luna-core/src/vm/dump/puc/puc_5{1..5}.rs`,
sharing `lower.rs` (5.1) with `classic.rs` (5.2/5.3) and `modern.rs`
(5.4/5.5); `lower.rs`'s module header states what the interpreter trusts.

## Known correctness gaps

None open as of v3.0.0,
and three classes of use-after-free that the arc started with are fixed
— including one that a previous release had judged impossible to
reproduce and which turned out to be two all-platform GC bugs.

Files excluded from the official-suite gate are listed in the `excluded`
arrays of `crates/luna-core/tests/official_run.rs`, each with an inline
rationale. They are scope choices, not gaps — for instance `gc.lua`,
`gengc.lua` and `tracegc.lua` make allocator-timing assumptions that do
not hold for a different GC, and 5.5's `files.lua` wants a real
`/dev/full`, which exists on Linux and not on macOS.

## Deliberate differences

Beyond the corpus, luna is compared against stock PUC with probes that
walk every standard-library function through missing, nil, wrong-typed
and numeric-string arguments, every library's surface in each dialect,
`collectgarbage`, call-stack levels, tracebacks, and language-level error
messages. What still differs does so on purpose:

- **Metamethod recursion depth.** PUC counts a metamethod call against its
  200-level C-call limit, so a `__index` function recursing about 200
  deep raises "C stack overflow". luna does not count metamethod calls;
  such recursion is bounded by the Lua stack (one million slots) and
  raises "stack overflow" there. It never crashes the process.
- **`pcall` nesting depth.** Both stop with "C stack overflow", but the
  depth at which they do depends on how many C levels the host has
  already used (PUC's standalone interpreter spends about three before
  the script runs), so the exact count differs.
- **Local time.** luna-core links no C library timezone code:
  `os.date` without a leading `!` formats UTC, and `os.time`'s valid
  range is computed rather than taken from the host's `mktime`.
- **`collectgarbage("step")`'s result** says whether the step finished a
  cycle. luna's collector is its own, so this follows luna's progress,
  not PUC's. Everything else about `collectgarbage` — options per
  dialect, return shapes, how parameters read back — matches.
- **Implementation-internal values** that the manual leaves open or that
  expose the compiler: `#t` on a table with holes may pick a different
  border; `debug.getlocal` past the declared locals reads temporaries
  whose contents depend on register allocation; a C function's
  return-hook `ftransfer` depends on its own stack use.
- **"too many registers" has no `near` token.** PUC raises it while
  parsing, with the lexer's current token at hand; luna allocates
  registers after the whole chunk is parsed, so the message stops before
  the `near` part.
- **Not reproduced: PUC bugs and C undefined behaviour.** PUC 5.1's
  compiler merging `0` and `-0` constants; 5.1 `io.lines(nil)` raising
  through a stack-index bug; out-of-range float-to-integer conversions
  (5.2 `string.format("%d", 2^63)` raises the range error the 5.2 test
  suite expects); a leaked pattern-matcher depth counter in 5.3's
  `gmatch`; results that depend on how the host C compiler or C library
  was built (fused multiply-subtract in 5.1/5.2 `%` on arm64, `%a`
  rounding in the macOS C library, 5.2's `%a` existing only when built
  with `LUA_USE_AFORMAT`).
- **Unavailable without a C library:** two-way `io.popen` modes on
  5.1/5.2, `os.clock` as CPU time (it measures time since the `Vm`
  started), locales other than C/POSIX, and loading C modules
  (`package.loadlib`, the C searchers).

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
