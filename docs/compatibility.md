# Compatibility

The compatibility surface for embedders deciding whether luna fits their
host. Current as of **v3.1.0** (2026-09-24). For performance methodology
and measured baselines see [`performance.md`](performance.md).

---

## How compatibility is established

Not by inspection. A private corpus of **807 fixtures** runs against
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
translators (see [below](#loading-puc-luac-files)), and `string.dump`
writes the running dialect's PUC bytecode, which the stock interpreter of
that version runs (see [`string.dump`](#stringdump-writes-puc-bytecode)).

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
Worked examples: `crates/luna-core/tests/it/macro_lua.rs` and
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

Covered — `crates/luna-jit/tests/it/capi.rs` is the conformance suite
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

### `string.dump` writes PUC bytecode

`string.dump(f)` returns a chunk in the running dialect's PUC format:
stock PUC 5.1.5, 5.2.4, 5.3.6, 5.4.9 or 5.5.1 loads it with `load` or
runs it as `lua file`. `string.dump(f, true)` (5.3 and later, where the
argument exists) drops debug information as PUC's `strip` does. luna
re-encodes each function into that version's instruction set; where the
dialect cannot express something luna's compiler produced, `string.dump`
raises `unable to dump given function` instead of writing a chunk that
would behave differently, for example a 5.1 function that uses its
environment table as a value (5.1 reaches globals only through
`GETGLOBAL`/`SETGLOBAL`). No diff_puc fixture hits a refusal.

Every diff_puc fixture is compiled by luna, dumped, and run by the stock
interpreter of its dialect, matching PUC running the source byte for byte
in stdout (and in the error text for `_err` fixtures); the stripped dump
matches PUC running the same program compiled by `luac -s`; and the dump
loads back into luna to the same result (`tests/it/puc_dump.rs`).

`load` accepts a chunk of the running dialect's own PUC version under the
same switch as luna's own format (below), since it is what `string.dump`
produces. MacroLua has no PUC format; its `string.dump` writes luna's own.

### luna's own dumps

`luna_core::vm::dump::dump` (used by `luna-aot`) writes luna's own binary
format: the running dialect's PUC header, then a `"\x00LunaV1\x00"`
sentinel and a body in luna's 65-op instruction set. It loads back into
luna, not into PUC.

Loading a luna dump, or a chunk of the running dialect's PUC version, is
gated by `Vm::set_bytecode_loading(false)` (on by default; the `sandbox`
builder turns it off). Every loaded chunk, in either format, is verified
before it runs; see "Verification on load" below. The verifier checks
structure, not everything a crafted chunk can reach, so a host taking
untrusted input should still close the gate.

### Loading PUC `.luac` files

A chunk of another dialect's PUC version needs
`Vm::set_puc_bytecode_loading(true)`, **off by default**. The
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

### Verification on load

**luna verifies every binary chunk when it is loaded; PUC Lua does not.**
PUC removed its bytecode verifier in 5.2, and its manual warns that
maliciously crafted binary chunks can crash the interpreter. luna runs one
verifier on the function tree either loader produces (its own dump format,
or a PUC chunk after translation) before handing it to the VM. There is no
switch to turn it off. A chunk that fails is refused by
`load` / `loadfile` / `dofile` / `Vm::load` with a load error of the form
(5.4 / 5.5 wording)

    binary string: bad binary format (function at line 3, instruction 5 (LoadK): constant 12 out of range (7 constants))

Every binary-load error, from the verifier or from reading a truncated or
foreign chunk, is worded as the running dialect's `lundump.c` words it,
behind `lundump.c`'s chunk name (`@`/`=` dropped, `binary string` for a
chunk loaded from a string under its default name):

| Dialect | Truncated | Other header | Refused by the verifier |
|---|---|---|---|
| 5.1 | `unexpected end in precompiled chunk` | `bad header in precompiled chunk` | `bad code in precompiled chunk (<detail>)` |
| 5.2 | `truncated precompiled chunk` | `not a` / `version mismatch in` / `incompatible` / `corrupted` + ` precompiled chunk` | `corrupted precompiled chunk (<detail>)` |
| 5.3 | `truncated precompiled chunk` | `not a` / `version mismatch in` / `format mismatch in` / `corrupted` / `<type> size mismatch in` / `endianness mismatch in` / `float format mismatch in` + ` precompiled chunk` | `corrupted precompiled chunk (<detail>)` |
| 5.4 | `bad binary format (truncated chunk)` | `bad binary format (` `not a binary chunk` / `version mismatch` / `format mismatch` / `corrupted chunk` / `<type> size mismatch` / `integer format mismatch` / `float format mismatch` `)` | `bad binary format (<detail>)` |
| 5.5 | as 5.4 | as 5.4, with 5.5's `<type> size mismatch` / `<type> format mismatch` names | `bad binary format (<detail>)` |

PUC has no verifier after 5.1, so the last column has no PUC counterpart
in 5.2+; the category is the nearest one `lundump.c` has, followed by
luna's detail.

It checks, for every function and nested function:

- the opcode of every instruction;
- every register an instruction touches (including the runs implied by
  calls, returns, `LoadNil`, `Concat`, `SetList`, varargs and loops)
  against the function's stack size, and the parameter count too;
- constant, upvalue and nested-function indices are in range (not the
  constant's type: a field or global access whose constant key is not a
  string indexes with that value, as `t[k]` does);
- that every jump, loop edge and skipped instruction lands inside the
  code, and that control cannot run off its end;
- instruction pairing: `LoadKx`/`SetList` and their extra argument (which
  is never executed), numeric and generic `for` prep/loop pairs,
  comparisons and tests followed by their `Jmp`;
- that instructions reading a variable number of values from the stack
  top directly follow the instruction that set it;
- line info (empty, or one entry per instruction) and each nested
  function's upvalue descriptors against its parent;
- the register of every named local against the stack size;
- function nesting, at most 250 levels deep (the compilers stop at 200).

The verifier does not check register *values*. The debug library can
change those from plain source too, for example `debug.setlocal` on a
`for` loop's hidden state. The instructions that rely on a value's type
check it when they run and raise a Lua error (see "Deliberate
differences"); keep the bytecode gates shut for input you do not trust
all the same.

Per-dialect translators: `crates/luna-core/src/vm/dump/puc/puc_5{1..5}.rs`,
sharing `lower.rs` (5.1) with `classic.rs` (5.2/5.3) and `modern.rs`
(5.4/5.5); `lower.rs`'s module header states what the interpreter trusts.

## Known correctness gaps

None open as of v3.1.0,
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
- **Compile-time limit errors have no `near` token** ("too many
  registers", "function or expression too complex", "too many upvalues"
  from 5.2 on). PUC raises them while parsing, with the lexer's current
  token at hand; luna allocates registers and resolves upvalues after the
  whole chunk is parsed, so the message stops before the `near` part.
- **Not reproduced: PUC bugs and C undefined behaviour.** `debug.getinfo(level, ">…")`
  before 5.4 treating the option string as the function (5.1 crashes;
  luna rejects the option, as 5.4 does); 5.1 `io.lines(nil)` raising
  through a stack-index bug; out-of-range float-to-integer conversions
  (5.2 `string.format("%d", 2^63)` raises the range error the 5.2 test
  suite expects); a leaked pattern-matcher depth counter in 5.3's
  `gmatch`; results that depend on how the host C compiler or C library
  was built (fused multiply-subtract in 5.1/5.2 `%` on arm64, `%a`
  rounding in the macOS C library, 5.2's `%a` existing only when built
  with `LUA_USE_AFORMAT`).
- **State the compiler never produces raises an error.** PUC reads the
  hidden state of a numeric `for` loop, the target of a table
  constructor and its list of to-be-closed variables without checking
  them, so `debug.setlocal` or a crafted binary chunk that changes them
  makes it print garbage, loop or crash. luna raises instead:
  `'for' state corrupted` for a loop whose hidden slots no longer hold
  numbers of the loop's kind (on 5.1/5.2, which have one number type, an
  integer or a float is still a number and the loop goes on, as in PUC),
  `attempt to index a <type> value` for a constructor whose table was
  replaced, and `'<close>' state corrupted` for a to-be-closed slot
  registered out of order (reachable only from crafted bytecode).
- **`debug.setupvalue` does not change a C function's upvalues** (5.2+
  let it). The standard library and embedder functions keep state there
  that they rely on (a `gmatch` iterator's position, a wrapped
  coroutine, a function pointer); `debug.setupvalue` on a C function
  returns nothing, as it does for an index out of range.
  `debug.getupvalue` still reads them.
- **Unavailable without a C library:** two-way `io.popen` modes on
  5.1/5.2, `os.clock` as CPU time (it measures time since the `Vm`
  started), locales other than C/POSIX, and loading C modules
  (`package.loadlib`, the C searchers).

## CLI

luna's own options:

| Flag | Behaviour |
|---|---|
| `--lua=5.X` | Dialect: 5.1 / 5.2 / 5.3 / 5.4 / 5.5 (default 5.5) |
| `--sandbox` | Open base/math/string/table/coroutine only; reject bytecode loading |
| `--budget=N` | Cap dispatched instructions before raising |
| `--no-jit` | Install `NullJitBackend` — interpreter only |
| `--profile` | Print trace-JIT counters when the script finishes |
| `-h`, `--help` | Print the help |

They may appear anywhere before the script. Everything else follows the
selected dialect's standalone interpreter, `lua.c`:

| Option | Behaviour |
|---|---|
| `-e stat` | Run `stat` (a chunk named `(command line)`) |
| `-l mod` | `require` the module into the global `mod` (5.1 only requires it). 5.4 on: `-l g=mod` stores it in `g`, and a `-suffix` of `mod` is left out of the global's name |
| `-i` | Enter the REPL after the script; implies `-v` |
| `-v` | Print the version line (5.1: on stderr) before anything runs; stdin is then not run as a program |
| `-E` | 5.2 on: skip `LUA_INIT`, and let the package library ignore `LUA_PATH` / `LUA_CPATH` (the registry's `LUA_NOENV` is true) |
| `-W` | 5.4 on: turn warnings on |
| `--` | Stop handling options; what follows is the script |
| `-` | Stop handling options and run stdin as the script |

The options each dialect's `lua.c` does not know, and the ones that must
stand alone given with more letters (`-vx`; `-Ex` from 5.3 on), print that
dialect's usage message. The `arg` table and the script's `...` are laid
out as `lua.c` lays them out. With no script and none of `-e`, `-i` and
`-v`, stdin is run as a program, or, when it is a terminal, the version
line is printed and the REPL starts.

`LUA_INIT` runs before the options' chunks (in 5.1 before the options are
even read, so it runs ahead of a usage message): its value is a chunk
named after the variable, or `@file` to run a file. From 5.2 on
`LUA_INIT_5_x` is taken first, and `-E` skips both.

Errors are reported as `lua.c` reports them. An uncaught error prints
`<argv[0]>: <message>` on stderr followed by the traceback of `lua.c`'s
message handler (5.1 calls the global `debug.traceback`; 5.2 on take
`luaL_traceback`, and a non-string error object goes through its
`__tostring` or becomes `(error object is a <type> value)`), and the
exit status is 1. A script or `-e` chunk that does not compile, a file
that cannot be opened (`cannot open <name>: <reason>`), and a bad option
(the dialect's usage message) are reported the same way, without a
traceback. `os.exit` exits with the status it is given. As in `lua.c`,
an error in a program read from stdin without `-` is reported but leaves
the exit status 0. `crates/luna-jit/tests/it/cli_errors.rs` pins each case
to the text the PUC interpreters print.

The REPL is `lua.c`'s as it runs when built without readline. It
prompts with `_PROMPT` / `_PROMPT2` (`> ` / `>> ` when unset) on stdout
and reads stdin a line at a time, sharing stdin with `io.read`; a line
longer than `lua.c`'s 512-byte buffer is read in pieces. It reads more
lines while a statement is incomplete. From 5.3 on a line is first tried
as `return <line>;`; through 5.4 a first line `=expr` means
`return expr`; 5.5 warns about a line starting with `local`. The results
go through the global `print` and an error is reported with its
traceback, without the program name. End of input ends it with a
newline. The cases `crates/luna-jit/tests/it/cli_repl.rs` and
`cli_repl_edges.rs` pin include the dialects' quirks, such as 5.3
reporting its `_PROMPT2` value when the input ends inside a statement.

Things that differ from `lua.c`:

- `-v` and the start of the REPL print luna's version line, not PUC's
  copyright line.
- The values a script or `-e` chunk returns are printed after it
  (`=> value`).
- `package.path` and `package.cpath` default to `./?.lua;./?/init.lua`
  and the empty string, as luna has no install prefix and loads no C
  modules.
- With the `repl-line-editor` feature and stdin a terminal, the REPL
  reads lines through a line editor in place of readline: tab completion
  against the globals, syntax highlighting, and history in
  `~/.luna_history`; Ctrl-C drops the statement being typed. 5.5's
  `lua.c` loads readline at run time and, when that fails, warns (seen
  with `-W`); luna loads no library and does not warn.

## Quick verification

```sh
# Whole suite
cargo test --release --workspace

# One PUC test file, on a chosen dialect
cargo run --release -p luna-core --example runone -- \
  --lua=5.5 crates/luna-core/tests/official/lua-5.5.1-tests/calls.lua

# Differential corpus vs stock PUC (needs the reference binaries)
cargo test --release -p luna-core --test it -- diff_puc::

# Sandbox walkthrough
cargo run --release --example sandbox_demo -p luna-jit

# Microbench vs PUC and LuaJIT (both must be on PATH)
cargo bench --bench cross_dialect
```
