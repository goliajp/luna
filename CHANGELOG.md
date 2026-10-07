# Changelog

All notable changes to luna will be documented in this file. Format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

The public stability contract for the 1.x line covers:

- `pub` items in `src/lib.rs`'s exported tree
  (`luna::vm::Vm`, `luna::runtime::Value`, `luna::version::LuaVersion`,
  `luna::frontend::*` parser surface)
- The `lua.h`-compatible C ABI under `src/capi.rs`
- Bytecode binary compatibility with PUC Lua per-dialect (`.luac`
  files load in and out)

Internal modules (JIT codegen, dispatcher hot-path internals, heap
internals) may change without notice within 1.x for performance
optimization.

---

## [Unreleased]

### Breaking

- `FrameMaterializeInfo` has a new field, `n_varargs` (the extra
  arguments of a vararg function a trace inlined), and is 16 bytes;
  `Vm::jit_push_inlined_frame` takes it as a fifth argument. AOT trace
  metadata is version 4. `CompiledTrace` has the fields `side_children`
  (the side traces wired to its exits) and `inline_kinds`;
  `side_trace_cache` maps a sentinel to an exit index.
  `AdoptRequest::side_parent` and `AdoptedTrace::side_parent` carry the
  parent's prototype. `TraceCompiler` has the hidden methods
  `failure_known` and `publish_failure`.

- Chunks in luna's own binary format (PUC header followed by the
  `LunaV1` body) no longer load: a table constructor's op now carries
  its size hints, and the body tag is `LunaV2`. Dump the source again
  with this version. PUC bytecode loads as before.
- C API: errors leave a C function at once, as in PUC. `lua_error`,
  `luaL_error` and every API function that raises (`lua_gettable`,
  `lua_call`, `luaL_checkinteger`, ...) jump back to the call that luna
  made into the C function, so the code after them does not run. Before,
  the C function ran on to its `return` and its results were dropped.
  Building `luna-jit` now needs a C compiler (the C half of the C API is
  compiled with the `cc` crate).
- C API: `lua_version` returns the dialect's version as a `lua_Number`
  (504.0), as PUC 5.4 does; it returned the `int` 505. 5.2 and 5.3
  headers map it to `luna_version_52`, which returns a pointer as theirs
  do. The C functions that may raise (`lua_error`, `lua_getglobal`,
  `lua_setglobal`, `lua_settop`, `lua_register`, ...) are no longer Rust
  functions in `luna_jit::capi`; `luna_jit::capi::LuaState` is a thread's
  state instead of a wrapper around `Vm`.
- `Vm` no longer has the fields `capi_stack` and `capi_cstr_pin`; each
  thread keeps the C API's stack in `Coro::host_stack`. `ContKind` has a
  `Host` variant (a C function's continuation), and `Coro` the fields
  `host_stack` and `host_state`.

- C API: `lua_setglobal` and `lua_getglobal` go through `_G`'s
  `__newindex` and `__index` and raise their errors as PUC does, instead
  of writing and reading `_G` raw and dropping the error. Writing a global
  while `_G` is read-only now fails with "Attempt to modify a readonly
  table". Inside a C function luna called, the error is thrown when that
  function returns (its results are dropped), so the `lua_pcall` around
  it returns `LUA_ERRRUN`; outside one it is unprotected, and like PUC
  the process prints `PANIC: unprotected error in call to Lua API (...)`
  and aborts.

- `TableError` has a new variant, `ReadOnly`, which `Table::set` and
  `Table::set_int` return for a read-only table; a `match` on
  `TableError` needs an arm for it. `Table::try_set_existing` returns
  `false` for a read-only table.

- `Vm::take_error_traceback` returns PUC's text as a whole: it starts with
  the `stack traceback:` line, and in 5.1 and 5.2 leaves out the middle of
  a deep stack where `luaL_traceback` (5.1: `debug.traceback`), run by a
  message handler at the error, leaves it out. It used to start with the
  first level's newline and elide as if the stack had no handler on top.
  The level lines themselves are unchanged and already listed C functions
  (`[C]: in function 'error'`, `[C]: in function 'string.gsub'`); the
  embedding guide now describes the format per dialect.
  `Coro::error_traceback` also starts with `stack traceback:`.

- `Vm::call_value_with_handler` is no longer a level of the stack: a
  traceback taken in its handler ends with the function the host called,
  as one taken under PUC's `lua_pcall` does, instead of with a
  `[C]: in ?` line. The `luna` command line keeps that line: it calls
  `Vm::call_value_with_handler_in_c` (hidden from the docs), the same call
  made from inside a C function of the host's, as `lua.c`'s `docall` runs
  inside `pmain`.

- `luna_core::jit::trace::ExitTag` has a `Bool` variant and `TraceRecord`
  the `index_slots`, `index_key` and `settings` fields; the frame-materialise helper
  `luna_jit_trace_materialize_frames` takes a third argument, the
  closure of each frame. Code that builds these types by hand or matches
  `ExitTag` exhaustively has to name the new parts.

- `TraceRecord` has a new field, `for_step_up`: whether the step of the
  numeric `for` loop that closes the trace was positive while it was
  recorded. Code that builds a `TraceRecord` by hand has to set it.

- The syntax tree in `luna_core::frontend::ast` no longer allocates per
  node. Every list in it (a block's statements, call arguments,
  expression lists, assignment targets, declared names, parameters,
  `function a.b.c` paths, table fields, `if` arms) is a `List<T>`, a
  range into a vector of the `Chunk`; read it with `chunk.list(l)`, which
  gives a `&[T]` (`chunk.block_stats(&block)` for a block). Identifiers
  and string literals are interned in `chunk.names`: `Name` is now
  `Name { sym: Sym, line: u32 }` (its text is `chunk.name(n)`), and
  `Expr::Str` holds a `Sym` (its bytes are `chunk.str(s)`). The arms of
  `Stat::If` are `IfArm { cond, then_line, body }` instead of
  `(ExprId, u32, Block)` tuples. The node types are `Copy` and lose the
  name type parameter they had (`Chunk<N = Name>` and so on), and
  `block_uses_vararg` takes a plain `&Chunk`. To build a tree by hand,
  add lists with `chunk.push_list(&items)` and names or literals with
  `chunk.names.intern(bytes)`. Migration: replace `block.stats.iter()`
  with `chunk.list(block.stats).iter()` (likewise for `args`, `exprs`,
  `targets`, `names`, `vars`, `params`, `path`, `fields`, `arms`),
  `name.text` with `chunk.name(name)`, and the bytes of `Expr::Str(s)`
  with `chunk.str(*s)`. `parse`, `parse_tokens`, `compile_chunk`,
  `walk_rhs_for_calls`, `metamethod_safe_for_index_lhs` and
  `rhs_calls_nothing_unknown` keep their signatures.
- `LocVar::name` and `UpvalDesc::name` are a `DebugName` instead of a
  `Box<str>`. A `DebugName` dereferences to `str` (so `&*lv.name`,
  `lv.name.to_string()` and `lv.name == "x"` keep working) and stores a
  name of up to 22 bytes without a heap allocation. Build one with
  `DebugName::from` a `&str`, `String` or `Box<str>`.
- `luna_core::frontend::token::Token` has three type parameters, the
  payloads of `Str`, `Name` and `MacroQuote`, with defaults
  (`Token<S = Vec<u8>, N = Box<str>, Q = Box<[TokenInfo]>>`), so code
  that names `Token` is unchanged; the type is also `Copy` when the
  payload types are.
- luna-tools (built from the repository, not published) drops the empty
  `repl-polish` feature with its `luna-repl-polish` stub binary, and the
  `mcode-disasm` feature with its unused `capstone` dependency. For line
  editing, build the `luna` binary with luna-jit's `repl-line-editor`
  feature.
- Safe functions no longer accept raw pointers they would dereference.
  `Heap::barrier_forward` and `Heap::barrier_back` take the parent as a
  `Gc<T>` instead of a `*mut GcHeader`, and `UserdataMarker::mark` takes
  a `Gc<T>` with `T: GcObject`. `GcObject` is a new sealed trait in
  `luna_core::runtime` that every GC object type implements (`LuaStr`,
  `Table`, `Proto`, `LuaClosure`, `Upvalue`, `NativeClosure`, `Coro`,
  `Userdata`), so marking any handle the runtime gave you compiles
  unchanged. Migration: pass the handle itself, `heap.barrier_back(t)`
  instead of `heap.barrier_back(t.as_ptr() as *mut GcHeader)`.
- `Gc::from_ptr` (hidden from the docs) is an `unsafe fn`: the pointer
  must be a live object the heap manages. Migration: keep the `Gc` the
  runtime handed out instead of rebuilding it from `as_ptr()`; where the
  pointer really comes from elsewhere, call it in an `unsafe` block that
  states why the object is alive.
- `JitHandle::call` and `JitHandle::call_with` are removed. They called
  the compiled code with whatever arguments they were given and without
  the JIT window the code's helper calls need. Migration: call the
  function through `Vm::call_value` (or the `Lua` facade), which runs
  the compiled code when the arguments fit it and the interpreter
  otherwise.
- The JIT hooks on `Vm` that the `luna_jit_*` helpers call (hidden from
  the docs) take checked types instead of raw words: `jit_spill_stack`
  takes a `Value`; `jit_stack_update_raw` is replaced by
  `jit_stack_slot_mut`, which returns the slot to write; the string
  accumulator works on owned buffers (`jit_str_buf_acquire` returns a
  `Box<Vec<u8>>`, which `jit_str_buf_release` takes back,
  `jit_str_buf_intern` takes `&mut Vec<u8>`, and `jit_str_buf_extend`
  takes the buffer and a `Gc<LuaStr>`); and
  `jit_op_tforcall` writes its results through `&mut i64`. The C ABI
  helpers keep their signatures. `JitVmRebindRestore::restore_fn` is a
  plain `fn`, and an `IntChunkCompiler::enter` implementation now only
  stores the Vm pointer it gets (luna-jit-helpers' `enter_jit_ptr` does
  that) instead of dereferencing it.

### Changed

- Strings hash as PUC 5.4 and 5.5 hash them, from the last byte to the
  first, so that with the same seed a table lays out its string keys —
  and `pairs` visits them — as PUC does. The order a table with string
  keys is visited in therefore differs from earlier versions.
- A trace answers 5.4's `#t` from the table's length limit inline, and
  reads `t[k]` for a string `k` it did not see at recording time by
  walking the key's chain inline before calling the runtime.

- On Windows the `luna` command keeps the MSVC C library's `FILE` for each
  file and standard stream, so that what PUC built with MSVC does with
  them, `luna` does too: `setvbuf` sizes and the positions `seek` then
  reports, a `write` right after a `read` failing and a `read` right after
  a `write` returning the stale buffer, `ungetc` at the start of a buffer
  dropping the byte, 5.1 and 5.2 reading numbers with that library's
  `fscanf`, standard output buffered in 4096-byte blocks on a pipe or file
  and written after every call on a console (so stdout and stderr
  interleave as with `lua.exe`), a Ctrl+Z typed on a console ending only
  its line, and `setvbuf` with a size below 2 ending the process with
  status 0xC0000409. Files opened with `b` follow the library too.
- On Windows the `luna` command reads and writes as `lua.exe` does, through
  the MSVC C library's text mode: its standard output, standard error and
  standard input, and files opened without `b`, write `\n` as `\r\n` and
  read `\r\n` as `\n`; a Ctrl+Z ends the input, and `seek` reports what
  that library's `ftell` does. `Vm::set_crt_text_mode` turns the same on
  for the files of any `Vm` (off by default, on every platform), and
  `luna_core::stdio::write_stderr` writes to standard error as the `luna`
  command does. 5.1 and 5.2 read lines on Windows in 512-byte pieces, the
  MSVC `BUFSIZ`, as PUC does there.
- The C library's `errno` is kept as PUC's process would have it, and a
  failure that sets none reports what an earlier call left there, as it
  does in PUC 5.1–5.3 on Windows (a `write` right after a `read`): a
  number that converts out of range (in `tonumber`, in arithmetic, in the
  lexer, in a constant `^` or `%` the parser folds), a math function out
  of its domain or range, `^` and a float `%` (also in compiled traces,
  compiled functions and luna-aot binaries), a failed open, remove or
  rename, and `os.time` beyond the C library's range each update it, by
  the rules of the Universal CRT on Windows and of glibc elsewhere; 5.4
  and 5.5 clear it where PUC does. `luna_core::cerrno` holds the value.
- On Windows a failed `io.open`, `os.remove`, `os.rename`, `loadfile` or
  `io.lines` reports the C library's message and number (`No such file or
  directory`, 2) instead of the system's (`The system cannot find the path
  specified.`, 3). Files are opened, removed and renamed through the
  system calls of that library: `os.rename` does not replace an existing
  file and `os.remove` does not remove a directory, as in `lua.exe`;
  `os.tmpname` names a file without creating it; `os.time` fails beyond
  the year 3000 and before 1970; 5.1's `tonumber(s, base)` clamps to the
  32-bit `unsigned long`.
- With `Vm::set_crt_text_mode` (the `luna` command on Windows), `io.open`
  in 5.1 reads its mode as the MSVC C library's `fopen` does: a `ccs=`
  selects that library's UTF-8, UTF-16LE or `UNICODE` text mode, with the
  byte order mark read or written as the file opens, and a mode it calls
  invalid ends the process with status 0xC0000409, as do line, number and
  `read(0)` reads of such a file; a `seek` after an odd number of bytes in
  a Unicode mode ends it with 0xC0000005. In every dialect, opening an
  empty file in a text mode with `+` leaves EINVAL in `errno`.
- `loadfile` of a file that opens but cannot be read says `cannot read`.
- On Windows the `luna` command takes file names, its command line and
  environment variables through the ANSI code page, as `lua.exe`'s narrow
  C functions do: a name given as UTF-8 bytes reaches the system as the
  code page reads those bytes, and `arg`, `os.getenv` and `os.tmpname`
  give the code page's bytes (`?` for a character it has none for).
  `Vm::set_global_bytes` sets a global whose name is not UTF-8.
- On Windows `seek` on standard input goes to the system as for any
  file, so a file redirected in can be sought and read again, as with
  `lua.exe`; it reported `Invalid seek` before.
- A UTF-8 stream of 5.1's `ccs=` reads bytes that are not UTF-8 as
  `MultiByteToWideChar` does (one U+FFFD for a lead byte with the
  continuation bytes it took, one for every other byte).
- 5.1's retry of a numeral with `strtoul` leaves ERANGE in `errno` when
  the value overflows (`unsigned long` has 32 bits on Windows).
- The LLVM backend (`--features llvm-jit`, `LUNA_JIT_BACKEND=llvm`)
  compiles traces with the same trace lowering as the Cranelift backend:
  traces start in the baseline tier, move to Cranelift's code once hot,
  and are compiled by LLVM (`default<O2>`, for the host CPU) on a thread
  of their own once they have stayed hot for 20 ms; the Vm switches to
  LLVM's code at the next entry of the trace after it is ready (with
  `LUNA_TRACE_TIER=optimizing`, LLVM compiles every trace at once). A
  program that runs for less than that pays nothing for LLVM, so the
  backend starts as fast as the Cranelift one. It now compiles and runs the same
  traces as the Cranelift backend, inlined calls, side traces and
  tables included; before, it compiled only loops of integer
  arithmetic and comparisons. `luna_jit_llvm::LlvmBackend` on its own
  is the method JIT only: its `TraceCompiler` compiles no trace, and
  luna-jit's backend compiles them through
  `luna_jit_llvm::compile_function`.
- The LLVM backend's method JIT runs LLVM's `default<O2>` pipeline for the
  host CPU too (it compiled unoptimized code), and every LLVM compile
  reuses one target machine per thread.
- Compiled traces divide by a constant (`x // 7`, `x % 7`) with a
  multiply instead of a division instruction, in every tier: the
  baseline and Cranelift tiers compile without an optimizer that would
  do it, and a 64-bit division takes tens of cycles (`s = s + i % 7` over
  3 million iterations: 5x faster). A table field read or written at the
  slot it was recorded in is checked with one compare-and-branch per
  condition (about 15% off a loop of `t.x = t.x + 1` in the Cranelift
  tier).
- `TraceCompiler::tier_up`: a backend that leaves something in
  `TierUp::source` is asked again, at the next entry of the trace once it
  runs code the backend returned, else after another `at` iterations.

- C API: the `io` library of a state made through the C API is C over the
  C library's stdio, as PUC's is, for every dialect: file handles are
  `luaL_Stream` userdata (5.1: a `FILE *`) with the registry's
  `LUA_FILEHANDLE` metatable, so `luaL_checkudata(L, i, LUA_FILEHANDLE)`
  accepts them and a stream a C library makes works with the io
  functions. Files are buffered by the C library, and `io.popen` uses
  `popen` (`_popen` on Windows). Rust-made `Vm`s and the `luna` command
  are unchanged.
- 5.1: every vararg function has the local `arg` after its fixed
  parameters, as PUC 5.1 built with `LUA_COMPAT_VARARG` has. It holds the
  table of extra arguments when the function does not use `...` (as
  before) and is nil when it does, so inside such a function `arg` no
  longer reaches a global `arg`, and the locals after it are numbered one
  higher for `debug.getlocal`.
- `load` with a reader function, and the C API's `lua_load`, parse a text
  chunk as the reader hands it over: the reader is called only when the
  parser moves past the end of what it has, as in PUC, so a syntax error
  stops the reading and the number of reader calls is PUC's. A 5.4+
  assignment to a `<const>` local is now reported by the parser, at the
  `=` or `,` after the target (PUC's line), not by the compiler.
- C API: `lua_dump` calls the writer once per block, in the order and
  sizes the dialect's `ldump.c` writes them (5.3 and 5.4 skip empty
  blocks, the others pass them), instead of once with the whole chunk.
- `HostContHooks` (hidden, used only by the C API) has a new field,
  `created`.


- The trace JIT compiles numeric `for` loops in the 5.1, 5.2 and 5.3
  dialects, which it used to leave to the interpreter, and float loops
  (`for x = 0, 1, 0.1`) in every dialect. Each dialect steps the loop as
  PUC does: 5.1 and 5.2 add the step to a float index and compare it with
  the limit, the comparison chosen by the step's sign; 5.3 does the same
  in integers, wrapping past `math.maxinteger`, or in floats. To keep
  such loops in the trace, traces also lower arithmetic between an
  integer and a float, `/` of two integers, float `//` and `%` (each
  dialect's modulo), ordered comparisons between an integer and a float,
  table reads and writes keyed by a float equal to an integer, and
  `string.sub` with such positions.

- 5.3: a trace checks the step sign of an integer `for` loop once, before
  the loop, instead of choosing the comparison with the limit on every
  iteration; a loop entered with a step of the other sign leaves the
  trace at its head.

- 5.1 and 5.2: traces add, subtract, multiply, take the modulo of and
  negate the integers the VM keeps for doubles (`#t + #u`, `#t * 2`,
  `i % #t`, `-#t`), which used to stop the recording. The trace keeps the
  exact result while it is within 2^53 of zero, where it is the double
  the operation gives, and otherwise leaves for the interpreter, which
  rounds as the doubles do and gives -0 for a zero product with a
  negative factor or a negated zero, and nan for a modulo by zero.

- A trace follows calls into other Lua functions and runs them inline:
  methods found through a metatable's `__index` table (`o:m()`), local,
  upvalue and global functions, nested such calls. The inlined code is
  checked against the callee's function prototype, so it keeps running
  when the closures are made again (each run of a chunk that defines
  methods); an exit inside an inlined function resumes the interpreter
  there with the call frames rebuilt. A call is inlined when it wants at
  most one result, passes a fixed number of arguments and calls a
  function that is not vararg; any other call still ends the trace.
  `redis_lua_shape`'s method_dispatch runs its whole loop in one trace.
- Booleans are a trace entry type: a loop whose registers hold `true` or
  `false` when it gets hot is compiled, and one trace serves both values,
  the branches on them guarded. `not`, boolean constants and comparisons
  with `true` / `false` stay in the trace, as does the jump over an
  `else` branch the recording did not take.

- The bytecode verifier rejects a `GETFIELD`, `SETFIELD` or `SELF` whose
  constant key is not a string. luna's compiler and its translation of
  PUC 5.1–5.3 bytecode only emit string keys there, but chunks that an
  older luna translated from PUC 5.1–5.3 and dumped may hold a numeric
  key in these instructions; load the original PUC chunk again instead
  of such a dump.
- Calling a Lua function on 5.2–5.5 sets only the missing parameters to
  nil, as PUC `luaD_precall` does, instead of clearing the function's
  whole register window (5.1 still clears it, as PUC 5.1 does). A
  compiled trace checks on entry only the registers it reads before
  writing them, so whatever the rest of the window holds no longer
  keeps it from running.
- `Vm::install_null_jit` now also switches the JIT off
  (`set_jit_enabled(false)` and `set_trace_jit_enabled(false)`), and so
  does the CLI's `--no-jit`, with or without `--sandbox`: before, hot
  loops and calls were still counted and recorded as traces that the
  no-op backend then failed to compile. Installing a real backend
  afterwards does not switch the JIT back on; call the two setters with
  `true` for that.
- The trace JIT compiles traces with Cranelift's `opt_level = "none"`,
  which cuts a trace's compile time by about a fifth with no measured
  change in the speed of the generated code.
- The trace JIT caches a trace that nothing can enter (not dispatchable
  and not reachable as a down-recursion or side trace) without
  generating machine code for it.
- The interpreter skips the per-instruction trace lookup on functions
  that hold no enterable trace, and the call trigger no longer scans a
  function's traces on every call once its entry is settled.
- A Vm with no JIT backend installed (`Vm::new`, `Vm::new_minimal`, the
  sandbox builder, any embedder that depends on `luna-core` alone) starts
  with both JIT flags off, so it no longer counts hot loops and calls or
  records traces that nothing could compile. `Vm::install_jit_backend`
  (and so `luna_jit::install_default_jit`) turns on each flag the
  embedder has not already set with `set_jit_enabled` /
  `set_trace_jit_enabled`. `jit_enabled()` and `trace_jit_enabled()`
  report `false` on such a Vm.
- Interpreter fast paths that PUC Lua also takes, with unchanged results:
  the dispatch loop tests one flag for an instruction budget, a memory cap
  or an armed hook instead of checking each on every instruction; integer
  `%` and `//` and the integer and float arithmetic and bitwise operators
  are computed in the opcode itself; table reads return a raw hit without
  entering the `__index` path, and string keys shorter than 41 bytes are
  matched by identity; table writes update a present key in place;
  `return` with zero or one value into a Lua caller skips the close and
  hook machinery when nothing needs it; growing a table's array part
  copies it in one block; and `#t` is answered from two counters when the
  array part holds exactly a leading run of values.
- A metatable remembers which metamethods it lacks (PUC Lua's `flags`
  cache), so looking up an absent `__index`, `__newindex`, `__eq`,
  arithmetic or other event costs one bit test; the table forgets them as
  soon as it gains any key. A string-keyed read that misses on a table
  follows table-valued `__index` links directly. Results are unchanged.
- Calling a native does less bookkeeping: whether it is `pcall`,
  `xpcall`, `pairs` or an async native is decided once when the closure
  is created, the running natives are kept in one list instead of two,
  the post-call collection check is a single comparison, and a return
  clears only the results the caller did not want.
- The trace JIT gives up a loop or function entry whose recordings keep
  running past the longest trace it builds, after three of them, as it
  already did for one whose traces keep failing to compile. Before, such
  a place was recorded again every time it turned hot: a function called
  in a loop paid for a recording on every call.
- With the trace JIT on, the interpreter does less per instruction when
  nothing is being recorded and the function has no enterable trace: the
  per-instruction checks test the condition that is almost always false
  first, and a numeric `for` counts its back-edges after stepping instead
  of re-reading its three control slots.
- `runtime::Frame` has a new public field, `ccmt: u8`: the number of
  `__call` metamethods resolved to reach the frame, which the Vm used to
  keep in a vector beside the frames.
- The interpreter runs the opcodes that only read and write registers,
  Lua-to-Lua calls and returns, and the return of a metamethod written in
  Lua to the instruction that called it, in a loop that keeps the program
  counter, the register window and the constants in locals; the frame is
  reloaded only after something that may have changed it (a metamethod,
  an error, a hook, a native that ran Lua code). A function holding a
  compiled trace hands an instruction to the trace dispatcher only at the
  pcs where a trace starts, no longer on every instruction.
- The compiler emits PUC 5.4's constant- and immediate-operand forms for
  every dialect: `x + 1`, `x - 1`, `x * 0.5`, `x % 7`, `x >> 2`, `x < 5`,
  `5 < x`, `x == 3` and `x == "s"` no longer load the constant into a
  register first (`AddI`, `SubI`, `AddK` … `BXorK`, `ShrI`, `ShlI`,
  `EqI`, `LtI`, `LeI`, `GtI`, `GeI`, `EqK`). A loop like
  `for i = 1, n do s = s + i % 7 end` runs three instructions per round
  instead of four. `string.dump` writes them as the dialect's own opcodes
  (5.4/5.5) or as `RK` operands (5.1–5.3). Arithmetic between an integer
  and a float, and `/` between two integers, are computed in the
  instruction itself instead of the general path.
- `runtime::Frame::tm` is now `Option<runtime::function::FrameTm>` (one
  byte) instead of `Option<&'static str>`: the frame records whether it
  runs a finalizer, a `__close` handler or another metamethod, not the
  event's name. `Frame` shrinks from 56 to 40 bytes.
- 5.4 and 5.5 build a new closure every time a function expression is
  evaluated, as PUC 5.4 and 5.5 do. luna used to reuse a prototype's last
  closure when its upvalues matched, which only PUC 5.2 / 5.3 do: two
  closures from one expression compared equal and collapsed into one table
  key on 5.4 / 5.5. 5.2 and 5.3 keep the cache.
- In 5.1, `_ENV` is an ordinary name, as in PUC 5.1, which has no `_ENV`:
  `_ENV = x` assigns the global `_ENV`, and a local named `_ENV` does not
  change where global names are looked up. luna used to bind the name to
  the hidden environment of the function, so `_ENV = x` replaced the
  environment of every function sharing it. `setfenv`, `getfenv` and
  `module` are unchanged.
- On 5.2–5.5 the functions of the standard library are never collected,
  as PUC's light C functions are not: the collector no longer visits them,
  and a weak table keeps one even after it was removed from its library.
  The memory of a library function a script deletes is released only when
  the Vm is dropped; the most this holds is the library's own functions,
  so it does not grow with run time. 5.1 still collects them, as PUC 5.1
  does: its library functions are ordinary collectable C closures.
- A table lookup by a short string key no longer goes through the
  general key-comparison walk, which saved and restored a dozen
  registers on every lookup: field reads and writes such as `t.x` run
  about 20 fewer machine instructions each.

### Fixed

- An exhausted instruction budget or memory cap now stays exhausted
  until the host arms a new one with `Vm::set_instr_budget` /
  `Vm::set_memory_cap`: every further instruction raises the same
  error again, inside `pcall` callers, `xpcall` handlers, `__close`
  and `__gc` handlers, metamethods, library callbacks and coroutines
  alike, and `Vm::error_kind()` stays `InstrBudget` / `MemoryCap`
  (which the cap did not set before). Before, the limit cleared itself
  when it fired, so `pcall(function() while true do end end) while
  true do end` ran forever once the inner loop had used the budget
  up, a handler or finalizer could run unmetered, and a script's `__gc`
  ran unbounded when the Vm was dropped. Every release from 1.1.0 to
  4.0.2 is affected. `Vm::instr_budget_remaining()` reports `Some(0)`
  once exhausted. While a budget or a cap is armed the method JIT is
  no longer entered either (traces already were not), so the JIT does
  not need to be switched off for sandboxed scripts.
- Unbounded nesting no longer crashes the process with a native stack
  overflow; it raises the Lua error PUC raises, which `pcall` catches.
  Nesting that runs on the native stack (metamethods, library callbacks
  such as `table.sort`'s comparator, `string.gsub`'s replacement and
  `load`'s reader, `tostring`'s `__tostring`, coroutine resumes,
  protected calls, message handlers, C API calls, the parser) is counted
  against PUC's 200-level C-call limit, failing at the depth PUC fails
  at in each dialect, and is also checked against the running thread's
  real stack bounds, so it fails with "C stack overflow" on an embedder
  thread with a 256 KB or 2 MB stack too. Metamethod calls were not
  counted before, and a coroutine now starts from its resumer's count,
  as `lua_resume` does. A function the method JIT compiled calls itself
  natively only while stack is left, then lets the interpreter make the
  remaining calls, so deep recursion ends with "stack overflow" at the
  Lua stack limit, or completes, as in PUC. The Lua stack limit is
  PUC's: 1,000,000 slots from 5.2 on, checked as `luaD_growstack`
  checks it, with 200 more for the message handler of the overflow and
  "error in error handling" past those; 5.1's is `LUAI_MAXCALLS` (20000)
  frames, doubling its frame array from 8 as PUC does. Frames sit where
  PUC puts them (vararg frames above the arguments, the function a
  protected call calls where `pcall` / `xpcall` put it, the message
  handler at the raising frame's top, the standalone interpreter's
  chunk under a `pmain` frame), so a recursion ends at the same depth in
  every dialect. "error in error handling" no longer runs the message
  handler again. The compiler walks a left-associative chain (`1 + 1 +
  ... + 1`, `a.b.b...`, `f()()...`) without recursion, so a chain of any
  length compiles, as in PUC; nested syntax too deep for the parser
  fails with the dialect's nesting error at PUC's depth (from 5.2 on a
  level per statement); a `for` loop's hidden control variables count
  against the 200 locals. `coroutine.wrap` in 5.4 and 5.5 no longer closes a coroutine
  that a resume refused to start. Seen in 4.0.2: a `table.sort`
  comparator, `string.gsub` replacement, `__tostring` or coroutine
  resume recursing without end on a 256 KB thread, a compiled function
  recursing without end on any thread, and a 200000-term sum on the main
  thread all crashed it.

- A trace leaving inside a function it inlined two or more calls deep
  rebuilt the middle frames with their callers' resume pcs, so such a
  frame went on at the wrong instruction once the inner call returned
  (`return h(x) + 1` lost the `+ 1`). Exits one call deep were right.
- The side-trace gate read a `Jmp`'s offset from the `sBx` field instead
  of `sJ`, so it took a backward jump of fewer than 256 instructions for
  no jump and let a side trace that loops and writes to tables compile.

- `#t` on a table with holes could return a different border than PUC.
  `{f(6), f(7), g(), f(8)}` (with `g` returning nothing) has borders 2
  and 4: PUC 5.5.1 returns 2, luna returned 4. Each dialect now sizes a
  table as its PUC does — the constructor's size hints, a list store
  growing the array to its last index, `table.pack` and vararg tables
  sized to their count, the rehash sizing of each version (5.5's
  differs), the placement of number and boolean keys in the hash part,
  5.1 to 5.3 adding a key assigned `nil` — and searches for the border as
  its PUC does: 5.1 to 5.3 by binary search, 5.4 from its length limit
  (which `#t` lowers and indexing past it raises), 5.5 from its length
  hint. The trace and method JITs and AOT code give the same answers.
  5.1 also hashes strings as PUC 5.1, which sizes tables with string
  keys the same way. All released versions, 4.0.2 included, are
  affected.
- In a table constructor, a call or `...` that is the last list item but
  is followed by keyed fields (`{f(), x = 1}`) now gives one value, as in
  PUC; it gave all of its values. All released versions, 4.0.2
  included, are affected.

- 5.3–5.5: the compiler folds constant `^`, `//`, `%`, bitwise operations
  and `~` as PUC's parser does (`2^53` is a constant, not a `POW` at run
  time), and leaves a negated float zero (`-0.0`) to run time as PUC
  does, so `string.dump` writes the same code and constants as PUC for
  them.

- `string.dump` of a main chunk describes its `_ENV` upvalue as PUC does
  (in the stack, index 0).

- `string.dump` writes the code PUC's compiler makes for operations on
  constants it cannot fold: in 5.1–5.3 both constants are the
  instruction's operands, with no load into a register (`1/0`, `3 - 3.0`,
  `5 % math.huge`); in 5.4 and 5.5 the right constant enters the constant
  table first and a constant on the left of `+` or `*` moves to the right
  (`7.5 // 0`, `2 * 0.0`); a constant on the left of any operator is
  loaded after the right operand is computed. 5.2 folds an operation on
  two constants that gives nan (`(-2)^0.5`), as PUC 5.2 does, and 5.1–5.3
  list constants in the order PUC's code generator adds them.

- A loop calling `math.fmod` was compiled into a trace that never ran:
  reading the function `math.fmod` was a value the trace could not type,
  so the trace was marked not enterable. Traces now compute `math.fmod`
  in place, as the library does: two integers (5.3+) give C's truncating
  remainder, -1 gives 0 and 0 leaves the trace for the interpreter to
  raise its error; otherwise the result is the interpreter's `fmod`
  (`luna_jit_fmod`), so two NaN operands give the NaN the interpreter
  gives.

- A loop trace that ran a whole pass and returned to its head through
  its own tail could put back the registers that pass wrote as they were
  before it: the return was matched by its pc to a guard that also
  leaves at the head, and restored with that guard's register kinds.
  `if i == 5 then r = u end` inside a `while` left `r` nil after the
  loop, with no error. Such a return now restores with the tail's own
  kinds. Seen after 4.0.2: 4.0.2, 4.0.1 and 3.2.2 give the right result
  on the same program.

- 5.4 and 5.5: `local x <const> = X and nil or K` (or with `false`) made
  `x` a variable; PUC makes it the compile-time constant K, after running
  X, so `x` has no `debug.getlocal` entry and an inner function using it
  has no upvalue for it (affects 3.1.0 through 4.0.2). luna now does the
  same.
- A function compiled by the method JIT returned a local that only a
  branch assigns (`local r ... if i == 5 then r = i end ... return r`) as
  the number 0 when the branch had not run, instead of nil, and
  arithmetic on it went on with 0 instead of raising. The compiler took
  the branch's write as done on every path. Now a local some path leaves
  nil is treated as nil where it is read. All dialects, default settings;
  3.2.2, 4.0.1 and 4.0.2 have it.

- 5.4 and 5.5: `x - (C and nil or 0)` (also with `false`, or any
  expression whose `and` ends in one of them) gave `-0.0` for `x = -0.0`
  where PUC gives `0.0` (affects 3.1.0 through 4.0.2). PUC's code
  generator reduces such an operand to the constant 0, so it runs
  `x - 0` as `x + 0`; luna now does the same. `C` still runs.
- On x86 Linux, a float `%` with two NaN operands in a compiled trace
  (runtime JIT and luna-aot) returned the NaN glibc's `fmod` picks
  instead of the one the interpreter and PUC pick (the larger
  significand, as gcc's inline x87 `fprem`), so `print` could show `nan`
  where PUC shows `-nan`. Compiled code now calls the interpreter's
  `fmod` through a new runtime helper, `luna_jit_fmod`.
- luna-aot with `clang-cl`: the C entry was compiled with `/Fo:<path>`,
  which clang-cl reads as an output path starting with `:`, and clang-cl
  dropped the empty `.lt_skix` / `.lt_chai` /
  `.lt_prix` sections that cl.exe keeps. A cross build to
  `x86_64-pc-windows-msvc` from Linux or macOS could not link before.
- A function whose loop held a shorter loop, called again and again,
  could run the outer loop's body fewer times than written, with no
  error. A trace started in the inner loop leaves through two exits that
  resume at a plain pc: back into the outer body, or out of the outer
  loop. A side trace recorded from one of them was then run on the
  other as well, so the frame resumed where that side trace was recorded
  and the rest of the outer body was skipped. `for i = 1, 3 do local w =
  0 while w < 1 do w = w + 1 n = n + 1 end end` in a function called
  2000 times left `n` at 2218 instead of 6000 in 5.1. A side trace now
  runs only on an exit that resumes where it was recorded. All dialects;
  introduced after 4.0.1, which gives the right counts.

- A `math.max` or `math.min` call compiled into a trace, left by the
  trace while its arguments were being computed (an argument that
  stopped being a number), made the interpreter call
  whatever the call's register held, raising "attempt to call field
  'max' (a number value)" instead of the library's argument error. The
  trace now leaves at the library lookup. Seen with numeric `for` loops in
  5.1 and `ipairs` loops in 5.4 / 5.5. Introduced after 4.0.1; 4.0.1,
  4.0.0 and 3.2.2 give the right error on the same programs.

- C API: in the return hook of a C function, `lua_getlocal` reads the
  function's whole stack as PUC leaves it there — its arguments, what it
  pushed and its results on top — and `lua_getinfo(L, "r", ar)` gives
  the results' place; the arguments used to be overwritten by the
  results.
- 5.1–5.3: the call hook of a tail call runs before the caller's frame is
  replaced, as in PUC, so the hook sees the caller one level up with its
  locals, and 5.1 names the called function.
- 5.2+: a function's implicit final return (and 5.4+ a `return` with no
  values) happens while its locals are still active, so a return hook
  reads them with `debug.getlocal` / `lua_getlocal`, and 5.5's
  `ftransfer` is the first register above them, as in PUC. A function's
  outermost block no longer ends with a separate `CLOSE` before the
  return.
- C API: a thread that died by an error keeps the error's status:
  `lua_resume` and `lua_status` report `LUA_ERRMEM` for a memory error,
  `LUA_ERRERR` for an error in a message handler and 5.2/5.3's
  `LUA_ERRGCMM` for a finalizer error, instead of always `LUA_ERRRUN`, and
  a thread that finished reports `LUA_OK`; `lua_pcall` reports
  `LUA_ERRMEM` as well. 5.3's `string.rep` of a string too large to make
  raises "not enough memory for buffer allocation" as an ordinary error,
  as 5.3's buffer does.
- C API: `lua_closethread` (5.4 `lua_resetthread`) on the main thread
  runs the `__close` of its to-be-closed slots, newest first, before
  emptying the stack, and returns the status of an error one raises.
- C API: a coroutine made by `coroutine.create` or `coroutine.wrap` gets
  its copy of the main thread's extra space when it is made, as
  `lua_newthread` gives it, not when C first sees the thread.


- A trace recording that an error left (a call raising inside `pcall`)
  was closed the next time the same function was entered, giving a trace
  that ran only up to the failing operation and returned to its start, so
  the program looped for ever with the trace JIT on. The recording is now
  dropped when an error unwinds through it. Introduced after 4.0.1;
  4.0.1, 4.0.0, 3.2.2 and 3.0.0 do not hang on the same program.
- A coroutine resumed from inside a call that cannot yield (a sort
  comparator, a `gsub` replacement, a host's `lua_pcall`) can yield
  again, as in PUC, where that restriction belongs to the thread that made
  the call.
- `coroutine.isyieldable(co)` is true for a dead coroutine in 5.4 and
  5.5, as in PUC.
- `load` accepts bytes after a binary chunk, as PUC does.
- When a state closes, finalizers run newest first, as in PUC.
- On x86 Linux, float `%` (5.3 to 5.5) and `math.fmod` (every dialect)
  with two NaN operands returned the first one, so `print` could show
  `-nan` where PUC shows `nan` (affects 3.1.0 through 4.0.2). PUC built by
  gcc there computes `fmod` with the x87 `fprem` instruction, which picks
  the NaN with the larger significand, or the positive one when the two
  differ only in sign; luna now picks the same one.
- On aarch64 Linux and macOS, 5.1 and 5.2 float `%` (`a - floor(a/b)*b`)
  and 5.2 `tonumber` with a base rounded twice, where PUC, which the C
  compiler builds there with a fused multiply-add, rounds once: in a
  sample of random float operands three results in four differed in the
  last digits or more, and a NaN result could have the other sign
  (affects 3.1.0 through 4.0.2). Constant folding of `%` in those dialects
  follows the same rule.
- C API: `lua_pcall` calls its message handler (`msgh`, 5.1's
  `errfunc`), which it used to ignore: the handler runs where the error
  was raised, before the stack unwinds, its first result becomes the error
  object, and when the handler itself fails the status is the dialect's
  `LUA_ERRERR` (5 in 5.1, 5.4 and 5.5; 6 in 5.2 and 5.3) with "error in
  error handling", as in PUC 5.1.5 to 5.5.1. A non-string error object is
  no longer turned into a string.
- C API: a C function called from Lua sees its own arguments at index 1
  on; values the host left below the call (a message handler, say) used
  to come first. The values on the C API stack are now kept alive by the
  collector. `luaL_loadstring` names the chunk by its source
  (`[string "..."]`), as PUC does, instead of `(load)`.
- The REPL prints a chunk's results by calling `print` from inside a C
  level, as `lua.c` does from `pmain`, so a `__tostring` that takes a
  `debug.traceback` or walks `debug.getinfo` sees the same levels as
  with PUC (`[C]: in ?` at the bottom).
- The `luna` command buffers standard output as C stdio does under
  glibc: line by line on a terminal, in blocks on a pipe or file, with
  5.2 on flushing after each `print` and 5.1 not. A script's output and
  its error messages (unbuffered, on standard error) now reach a shared
  pipe, file or terminal in the same order as with PUC; in 5.1 a script
  that printed and then failed showed its output before the error, PUC
  after it. `io.stdout:setvbuf` changes that buffering, and
  `io.stdout:seek()` writes out what is still buffered first, as `fseek` does.
- Pointer texts follow the C library of the platform luna is built for,
  as PUC's `%p` does: `tostring` of a table, function, thread or
  userdata, `file (...)`, and `string.format("%p")`. A NULL light
  userdata prints as `userdata: (nil)` with glibc (as on Redis),
  `userdata: 0` with musl, `userdata: 0x0` on macOS, and Windows
  pointers are 16 upper-case hex digits without `0x`. They used to be
  Rust's `0x...` everywhere. `string.format("%p")` of a NULL light
  userdata is `(null)`, as in PUC.
- With an instruction budget armed (`Vm::set_instr_budget`,
  `with_instr_budget`), a loop compiled into a trace ran without using up
  the budget, so `for i = 1, 1e9 do end` outran it once the trace JIT
  compiled the loop. Traces are no longer entered while a budget is
  armed.
- Numeric `for` in 5.1 and 5.2 ran as an integer loop when both its
  initial value and its step were integers the VM keeps (results of `#`,
  `select('#', ...)`, string lengths), and then followed 5.3's rules: a
  float limit was rounded down, so with a zero step an index of 1 and a
  limit of 1.5 looped for ever instead of not at all, and a NaN limit with
  a zero or negative step looped for ever instead of not at all. These
  dialects have only doubles, and such a loop is now a float loop as on
  PUC. In 5.3, a zero step with a NaN or `-math.huge` limit now starts
  the index at 0, as PUC's `forlimit` does.
- Wrong values from the trace JIT when a loop keeps a table it builds in
  an iteration (affects 1.3.0 through 4.0.1, on 5.4 and 5.5): after
  `last = t` or `prev = {n = i}` in a numeric `for`, the variable held
  the table of the iteration the trace was recorded on once the loop
  ended (`last.n` was 66 instead of 400), and so did the next
  iteration's read of it. A loop that left a trace early (`break`, a
  failed guard) with a table holding only named fields live, or with a
  table copied to a second local, gave that local a stale value too.
  Such a table is now built for real: when a variable outside the loop
  holds it at the end of the iteration, when an operation the trace
  does not track reads it (`t and t[1]`, `m[t] = v`), and, at an early
  exit, in every register that holds it. AOT binaries built from the
  same traces had the same fault.
- An error raised by a native the host calls directly (`vm.call_value`
  on `error` or another library function, with no Lua function between)
  left no traceback for `take_error_traceback`; it now has one, whose only
  level is that native.
- `luna-aot compile` on a Windows host with the MSVC Rust toolchain
  failed with "MSVC C compiler not on PATH" unless it was started from a
  Developer Command Prompt. It now finds `cl.exe` and `link.exe` in the
  newest Visual Studio or Build Tools install and sets the `INCLUDE`,
  `LIB` and `PATH` they need itself, as the `cc` crate does. Looking up
  `clang-cl` and `lld-link` on `PATH` also missed the `.exe` names on
  Windows, and the linker lookup could take a coreutils `link` (Git for
  Windows, or `/usr/bin/link` on Unix) for the MSVC linker; a bare
  `link` is no longer considered. A host build of luna-aot for MinGW
  links with `gcc`, which MinGW ships, instead of `cc`; a MinGW target
  on a Windows host falls back to `gcc` when
  `x86_64-w64-mingw32-gcc` is not on `PATH`.
- For a Windows target, `luna-aot compile` writes `<out>.exe` when the
  output path has no extension, for MSVC as MinGW's gcc already did, and
  compiles the C entry with `/MD` so it uses the same C runtime as the
  Rust staticlib.
- A binary built with `luna-aot compile --dialect 5.1` (or 5.2, 5.3, 5.4,
  `macrolua`) ran its script on a Lua 5.5 `Vm`. A 5.3 or 5.4 binary
  stopped at startup with "PUC bytecode loading is disabled"; a 5.1 or
  5.2 binary ran with the 5.5 library and reported `_VERSION` as
  "Lua 5.5". The generated `main` now passes the dialect to
  the runtime entry, which creates the `Vm` for it. Affects every release
  with `--dialect`, 1.3.0 through 4.0.1.

- The table-field inline cache of a trace compiled ahead of time compared
  the cached node's key with the address the key had in the process that
  compiled it, so it never hit; it now reads the key from the slot the
  deploy side fills, like the trace's other string keys.

- A trace whose entry reads a register holding a boolean could never be
  entered, yet it was compiled and kept its loop or function head, so no
  trace ever ran there. Booleans now enter traces (see Changed); a
  recording that reads a value no trace is entered with (a coroutine, a
  userdata) is no longer compiled, and the head is recorded again once
  those registers hold other values.
- A Vm with no JIT backend that ran `eval_async` before
  `install_jit_backend` kept the method JIT off afterwards: the future's
  temporary switch-off counted as the embedder's own choice.
- Use after free in the collector: values a returned function left in
  stack slots above its caller's registers (closures, strings, userdata)
  could be freed by one collection and then marked by a later one, since
  the main thread's stack is marked whole while a coroutine runs and a
  suspended coroutine's stack is marked whole too. The collector now
  clears the running stack above the live registers when marking ends,
  and roots the whole running frame while a Rust debug hook runs (PUC's
  `luaD_hook` does the same), so a hook that collects cannot free a
  register written after the last safe point.
- With the JIT on, a 5.1 / 5.2 function that the method JIT compiled and
  that stored into a table under a NaN key went on silently instead of
  raising "table index is NaN" as the interpreter does.
- `os.time` (5.3 and later) now writes the normalised fields back into
  its table before raising "time result cannot be represented" for a
  time of exactly -1, as PUC does; a time past the range of `tm_year`
  (5.4 / 5.5) writes the fields back as given.
- `os.time` ignored the table's `isdst` field. A true `isdst` (any value
  other than nil and false) now moves the result one hour back, as PUC
  5.1–5.5 on glibc do in UTC, which has no daylight saving time; the
  fields written back (5.3 and later) are those of the shifted time.
- On Windows the machine code the JIT compiled for a `Vm` is now returned
  to the system when the `Vm` drops, as on the other platforms. 3.2.1 kept
  it there because Cranelift 0.124 allocated the pages with `VirtualAlloc`
  and never released them; Cranelift 0.136, which 4.0.0 moved to, maps
  them as a section that freeing unmaps. A Windows test checks that the
  memory committed to the process stays flat while `Vm`s that compile
  code are created and dropped.

### Added

- Traces inline calls into vararg functions, calls that want several
  results or all of them, and calls that pass a variable number of
  arguments, and create closures inside inlined functions; the baseline
  and Cranelift tiers and AOT binaries all do.
  `Vm::trace_inline_kind_dispatched_count` counts the dispatches of
  traces holding each kind.
- Side traces start at hot exits inside functions a trace inlined.
  `Vm::trace_side_trace_run_count` and
  `Vm::trace_side_trace_inlined_run_count` count their runs.
- The Vms of one `Engine` share the trace recordings that failed to
  compile: a recording another Vm already failed to compile (same code,
  start, entry tags and path) is not compiled again
  (`Vm::trace_shared_failures_known`, `Vm::trace_shared_failures_counted`).

- `luna_jit::install_llvm_backend_with` and `jit_backend::LlvmBackend`'s
  `llvm_after` (default `jit_backend::LLVM_AFTER`, 20 ms; `None`: LLVM
  compiles hot traces at once). `LUNA_TRACE_IR_DUMP=1` /
  `LUNA_TRACE_ASM_DUMP=1` also print what the LLVM backend compiles.
- `luna_jit::install_llvm_backend` (with `--features llvm-jit`) installs
  the LLVM backend on a `Vm` regardless of `LUNA_JIT_BACKEND`.

- `luna_core::runtime::mem`: the allocation context a `Vm` takes its memory
  from (`MemOwner`, `MemCtx`), with containers that allocate through it and
  report a failed allocation instead of ending the process (`LVec`,
  `LSlice`, `LBox`). A context uses the system allocator (the default), a
  host allocation function with PUC's `lua_Alloc` contract
  (`MemOwner::raw`, `unsafe`), or a safe `MemoryPolicy` that sees and may
  refuse every allocation (`MemOwner::policy`; `MemoryLimit` caps the bytes
  in use). `Vm::new_with_mem` / `Vm::new_minimal_with_mem` build a Vm on
  one; `Vm::memory_in_use` reports what a host function or policy has seen.
- C API: every object of a state (strings, tables, functions, userdata,
  threads, prototypes, upvalues) is allocated through `lua_newstate`'s
  allocation function, with PUC's object type as the old size of a new
  block; `lua_setallocf` moves later allocations and frees to the new
  function; `lua_gc(LUA_GCCOUNT/LUA_GCCOUNTB)` and `collectgarbage("count")`
  report the bytes the function has handed out.
- The memory inside objects comes from the Vm's allocation context too:
  tables' array and hash parts, prototypes' code, constants and debug
  records, closures' and native functions' upvalues, coroutine stacks and
  frames, the C API's per-thread `lua_State` and its userdata blocks.
- And so does the rest of what a Vm keeps: the string table, the
  collector's gray stack and finalizer queues (a gray stack that cannot
  grow is made up for by walking the object list, so a collection never
  fails), the stacks of running natives and host roots, and everything the
  parser and the compiler build while loading a chunk. A load that runs out
  of memory fails with "not enough memory"; the C API's `lua_load` returns
  `LUA_ERRMEM` for it.
  The parser and the compiler do not check each allocation: one that fails
  unwinds to the load, which drops what it built (giving every block back)
  and returns the memory error. This needs `panic = "unwind"`; built with
  `panic = "abort"` (the default for `wasm32` targets) a load that runs out
  of memory ends the process, as a standard library container would.
- luna-aot links `x86_64-pc-windows-msvc` without Visual Studio, on
  Linux, macOS or Windows: `clang-cl` and `lld-link` from LLVM with the
  MSVC C runtime and Windows SDK from `xwin splat`, named by
  `LUNA_AOT_MSVC_SYSROOT` or found in `cargo xwin`'s cache directory.
  The runtime staticlib's cargo build gets the same compiler, linker and
  sysroot. See docs/aot.md §3.
- The C API covers PUC's `lua.h`, `lauxlib.h` and `lualib.h` for all
  five dialects: headers in `crates/luna-jit/include/lua5.1` to
  `lua5.5`, with which a host built for one PUC version gets a state of
  that dialect from `luaL_newstate`. Threads and `lua_resume`, yields and
  continuations from C (`lua_yieldk`, `lua_callk`, `lua_pcallk`, 5.2's
  `lua_getctx`), `lua_load` and `lua_dump` with readers and writers,
  userdata with user values, metatables, `lua_arith` / `lua_compare` /
  `lua_concat` / `lua_len` / `lua_next`, warnings, `lua_gc` with every
  option of each version, the debug interface and C hooks, the whole
  auxiliary library (`luaL_Buffer` in both layouts, references,
  `luaL_traceback`, `luaL_requiref`, ...) and every `luaopen_*`. See
  `docs/compatibility.md`.
- A test that a binary built by `luna-aot` writes NaNs as the interpreter
  does (`aot_nan_sign`, all five dialects).

- C API: `luna_newstate(version)` makes a state for any dialect by its
  `LUA_VERSION_NUM` (501 to 505); `LUA_ERRERR`; Redis's
  `lua_enablereadonlytable(L, idx, enabled)`.
- `luna_core::stdio`: `use_c_stdout` makes standard output behave as C
  stdio's `stdout` (what the `luna` command does); `write_stdout` and
  `flush_stdout` write and flush it either way.

- Read-only tables: `Vm::set_readonly(t: Gc<Table>, on: bool)` marks a
  table read-only or writable again (Redis's `lua_enablereadonlytable`),
  `Table::is_readonly` reads the mark, and the facade has
  `LuaTable::set_readonly`. Every write to a read-only table raises
  `Attempt to modify a readonly table` in every dialect, with or without
  the JIT: assignments (with the position of the assignment), a
  `__newindex` chain that reaches the table, `rawset`, `setmetatable`,
  `debug.setmetatable`, the stores of `table.insert`, `table.remove`,
  `table.sort` and `table.move`, and 5.1's `package.seeall` (these with
  no position). A trace compiled before the table was marked leaves the
  store to the interpreter, which raises. `Vm::set_global` refuses the
  write too, and `Vm::table_error` turns a `TableError` from a raw
  `Table::set` into the error the interpreter raises. Reads cost nothing
  extra. An interpreter store tests one bit of the table header before it
  writes, the bit its write barrier tests (set for a black table as well
  as a read-only one), which costs one instruction per store; a trace
  tests a table it keeps storing into once per run, and the method JIT
  tests the tables a compiled function is given once per call. See the
  embedding guide, section 5.1.
- `luna_core::runtime::table::jit_layout::{TABLE_READONLY_BYTE_OFFSET,
  TABLE_READONLY_BYTE_MASK}`: where the JIT's inline stores find the
  read-only bit.
- `luna_runtime_helpers::luna_aot_run_dialect`, the C entry an AOT
  binary's `main` now calls with the dialect the script was compiled
  for; `run_bytecode_as`, its Rust counterpart; `dialect_code` and
  `dialect_from_code` for the numbers it takes. `luna_aot_run` and
  `run_bytecode` keep running a 5.5 dump. `luna_aot::runtime_stub::aot_main_as`
  takes the dialect the same way.
- `luna_jit::Engine`: VMs built through one engine (`engine.new_vm`,
  `engine.new_minimal_vm`, `Lua::with_engine`) share the traces and
  method-JIT functions they compile. A VM reaching code of the same
  content installs the compiled code another VM produced instead of
  recording and compiling it: a fresh VM running the token-bucket
  benchmark compiles nothing. The engine is `Send + Sync`; code is shared
  only between VMs of one dialect and the same trace settings, and each
  VM copies what it installs into its own code memory. New:
  `Vm::new_minimal_with_hash_seed`, `Heap::with_seed`, `Heap::seed`,
  `Vm::trace_adopted_count`, `luna_jit::jit::chunk_adopted_count`.

- `luna_aot::embed::compile_and_link_with` and `AotOptions`: the same
  build as `compile_and_link`, with the harvest diagnostics switched on by
  a field instead of the `LUNA_AOT_HARVEST_PROBE` environment variable
  (`compile_and_link` still reads it).
- `luna_core::runtime::table::jit_layout::{TABLE_ACOUNT_OFFSET,
  TABLE_APREFIX_OFFSET}`: offsets of the two array-part counters behind
  `#t`, which the method JIT's inline array stores keep up to date.
- `LuaVersion::has_closure_cache`: whether the dialect reuses a
  prototype's last closure (5.2 / 5.3).
- `Vm::closure_from_proto`, hidden from the documentation: the closure
  constructor the JIT helpers share with the interpreter.
- `Proto::has_dispatchable_trace`, `Proto::trace_call_head_settled` and
  `luna_jit::jit_backend::trace::trace_codegen_count`: hidden from the
  documentation (`#[doc(hidden)]`); they exist for luna's own tests and
  are not part of the supported API.
- A baseline code generator for traces on x86-64 and on aarch64 (except
  Windows): a trace is first compiled from the same lowering into a
  compact instruction list, given registers by one linear scan and
  encoded directly, without Cranelift, which takes it over once its loop
  has run 16384 iterations (4096 once the function it is in has been
  called again). `Vm::set_trace_tier` / `Vm::trace_tier`
  (`TraceTier::Auto`, the default, `Baseline` or `Optimizing`),
  `Vm::set_trace_tier_up_at` and `Vm::trace_tiered_up_count` choose and
  observe it per Vm; a new Vm starts from `LUNA_TRACE_TIER`
  (`baseline`, `optimizing`, anything else is `auto`).
- `Vm::set_field_ic_enabled` / `Vm::field_ic_enabled`: turn the trace
  JIT's table-field inline cache on or off for one Vm. A new Vm starts
  from `LUNA_JIT_FIELD_IC` as before.
- `luna-soak --vm-churn` (luna-tools, not published): creates a JIT Vm
  per iteration, runs the workload until the method JIT and the trace
  JIT have both compiled code, and drops it; `--max-second-half-rss-drift-pct`
  fails the run when RSS grows more than that from the middle sample to
  the last. The report (schema 2) records the mode, the Vm count per
  sample and the second-half drift.

---

## [4.0.2] — 2026-10-04

A memory-safety fix in the trace JIT. With the JIT on (the default for
`luna_jit` VMs and the `luna` binary), a recursive function that called a
value held in an upvalue could crash the process. Turning the JIT off
(`--no-jit`) was not affected. No public API changes.

### Fixed

- A trace through a recursive function typed a value read from an
  upvalue as a Lua function whenever the trace later called it, without
  checking the value. When it was a table with a `__call` metamethod, or
  a native function such as `math.abs`, and the trace left at that call,
  the table or native was written back to the register as a Lua
  function, and the interpreter then ran it as one: a segmentation fault
  in release builds, a heap-buffer-overflow under AddressSanitizer. A
  ten-line script triggered it with the default JIT settings in every
  dialect, 5.1 through 5.5. The trace now checks that the value is a Lua
  function where it reads it; when it is not, the trace leaves before
  the read and the interpreter performs the read and the call. Versions
  from 1.1.0 on may be affected; 2.18.0 through 4.0.1 are confirmed.

## [4.0.1] — 2026-10-01

Two trace JIT fixes. With the JIT on (the default for `luna_jit` VMs and
the `luna` binary), a loop that has run hot could compute a wrong value,
and in one case crash. Turning the JIT off (`--no-jit`) was not affected.
No public API changes.

### Fixed

- A loop variable that holds a different type on different iterations
  could come out of the loop with the wrong value, in every dialect. For
  example, a `while` loop that sets `x = 7` on odd iterations and
  `x = 0.5` on even ones returned `4602678819172646912` (the bits of
  `0.5` read as an integer) instead of `0.5`. When the variable switched
  between a table and a number, the table could be lost or an integer
  could be handed to the garbage collector as a table, which crashed the
  process (segmentation fault) at the next collection. A compiled loop
  now repeats only while every variable keeps the type it was compiled
  for, and otherwise returns to the interpreter for that iteration.
- In Lua 5.1 and 5.2, `math.min` and `math.max` inside a hot loop
  returned an integer where these dialects return a float. The result
  could print as a tiny float such as `4.4465908125712e-321` (an integer
  `900` read as a float), and `1 / -math.min(#t, 5)` with an empty `t`
  gave `inf` instead of `-inf`. They now return a float, as the
  interpreter does.
- AOT builds of Lua 5.3 code lowered traces with some of the 5.4 rules;
  they now use the 5.3 rules, as the JIT does.

---

## [4.0.0] — 2026-09-29

A major version for three reasons: luna-jit and luna-aot expose a few
functions whose signatures use Cranelift types, so moving Cranelift to a
new major version breaks them; `luna_aot::embed::TargetSpec` has public
fields of `object` types, which move from 0.36 to 0.40; and the new
Cranelift raises the minimum Rust version to 1.96. The `luna-core`
embedder API gains one constant and is otherwise unchanged. Several trace
JIT fixes below change results that were wrong: with the JIT on, a
program now computes what the interpreter computes.

### Added

- `luna_core::runtime::string::jit_layout::STR_SHORT_OFFSET`, the offset
  of the flag that marks an interned (short) string, next to the table
  offsets in `runtime::table::jit_layout`. The trace JIT reads it to
  compare two strings without calling back into the runtime.

### Changed

- Cranelift 0.124 → 0.136 (the `cranelift*` dependencies of luna-jit and
  luna-aot). The JIT and AOT code generator now also includes upstream
  fixes and optimizations from the last year, among them the aarch64
  addressing-mode fix for CVE-2026-34971.
- Minimum supported Rust version is now declared: `rust-version = "1.96"`
  on every crate, the version Cranelift 0.136 requires.
- The public functions whose signatures carry Cranelift types are now
  `#[doc(hidden)]`. They are internal between luna crates and not
  covered by semver:
  - `luna_jit::jit_backend::lower_int_chunk_into`
  - `luna_jit::jit_backend::trace::lower_trace_into`
  - `luna_jit::jit_backend::trace::lower_trace_into_named`
  - `luna_aot::embed::TargetSpec::cranelift_isa_builder`
- `object` 0.36 → 0.40 in luna-aot and luna-tools, the version
  cranelift-object already uses, so a build now carries one copy of it.
  The public fields `format`, `arch` and `endian` of
  `luna_aot::embed::TargetSpec` have the `object` types
  `BinaryFormat`, `Architecture` and `Endianness`, so code that builds or
  reads a `TargetSpec` field by field needs `object` 0.40 too.
- `syn` 2 → 3 in luna-jit-derive. The derive macros accept and generate
  the same code; a build that also has proc-macros on `syn` 2 (clap,
  serde, thiserror) now compiles both.
- `rustyline` 14 → 18 for the `luna` binary's `repl-line-editor`
  feature. The line editor now shows no colours when `NO_COLOR` is set.
- `inferno` 0.11 → 0.12 behind luna-tools' opt-in `flame-graph`
  feature. It brings quick-xml 0.41, which fixes RUSTSEC-2026-0194 and
  RUSTSEC-2026-0195.
- `inkwell` 0.9 → 0.10 in luna-jit-llvm (luna-jit's `llvm-jit`
  feature), still on LLVM 18.1 through the `llvm18-1` feature.
- AOT trace data sections are named by the target's object format
  rather than the host's, so a trace object built for Windows on another
  host gets the short COFF section names the deploy side looks for.

### Fixed

- The trace JIT compared two values by their raw bits whatever their
  types, so a traced loop took the wrong branch of `x == nil` when `x` was
  the integer 0 (both are stored as zero bits), and an integer equal to a
  table's address equalled that table. The same comparison skipped `__eq`
  for two tables, found two equal long strings unequal, and ordered
  strings with `<` / `<=` by address. Values of different types now
  compare unequal, two tables with a metatable or two long strings leave
  the trace so the interpreter compares them, and string ordering is no
  longer compiled. A table the trace built in the loop and dropped before
  the back edge was compared by what its register held before (it was
  never allocated). Every dialect was affected, and so is 3.2.2.
- A recursive trace whose body holds a value of a type the trace JIT
  could not work out (and is therefore never to be entered) was entered
  anyway: closing it as a recursive trace marked it runnable again.
  Recursive traces are only recorded with `Vm::set_self_link_enabled`.
- A generic `for` over `ipairs` whose values change type could hang
  under the trace JIT: when the loop body was nothing but the iterator
  call, the value-type check left the trace at its own first
  instruction and the dispatcher entered the trace again at once, over
  and over. Such an exit now lets the interpreter run that instruction
  first. Lua 5.1 to 5.3 were affected.
- The optional LLVM backend (`llvm-jit` feature) had the same raw-bits
  comparison: its traces and compiled functions treated nil as the
  integer 0, a compiled function returning nil returned 0, and an
  upvalue of another type was read as an integer. A compiled function
  also called itself where its code called a function through an
  upvalue that no longer held it. These cases now stay in the
  interpreter.

### Removed

- The patched Cranelift fork luna's own builds used (a git submodule
  redirected through `[patch.crates-io]`). Published crates never used
  it; luna's tests now build against the same crates.io Cranelift that
  users get.

---

## [3.2.2] — 2026-09-29

### Fixed

- On aarch64, a function compiled by the JIT could run the machine code
  of an earlier function instead of its own. Since 3.2.1 a dropped `Vm`
  frees its JIT code and the next compile reuses that memory, but the
  instruction cache was not invalidated for the new code, so a core could
  still hold the old instructions; a Lua 5.3 function returning `99` was
  seen to return `Int(4636666922610458624)`, the bits of a 5.2 build's
  `99.0`. The JIT now invalidates the instruction cache for every range
  of code it writes.

---

## [3.2.1] — 2026-09-28

Fixes found by a new fuzz target that runs generated hot loops with the
JIT on and off and compares the output.

### Fixed

- An assignment `b = a` right after a statement that assigned `a` (for
  example `a = a + 1`) dropped that assignment: the compiler wrote the
  result straight into `b`, so `a` kept its old value and a loop counting
  with it never ended. Every dialect was affected, with or without the JIT.
- The trace JIT left a table built by a trace unallocated when the trace
  ended at a call that runs before the table is used, as in
  `s[#s + 1] = {f()}`; the interpreter then filled a register that held
  no table, and the process could crash.
- A trace recorded from a loop nested in a numeric or generic `for`, and
  closed at the outer loop's back edge, went back to its own start instead
  of the outer loop's body, skipping the code before the inner loop (a
  `while` inside a `for` stopped running and the `for` ended early).
- A side trace recorded where a side trace left was wired to the parent
  trace's exit with the same number, which resumes elsewhere, replacing
  the side trace there. With a `while` loop inside a `for` in a function
  called often, leaving the `while` then returned from the function, so
  the rest of the `for` loop did not run.
- Compiling a trace with a `math` call that is checked once before the
  loop hit a debug assertion of the code generator, so debug builds
  panicked on such loops.
- The machine code the JIT compiled for a `Vm` was never freed, so a host
  that creates a `Vm` per request grew by the code of every one of them.
  It is now freed when the `Vm` drops, and a trace or function that fails
  to compile frees its partial code at once. `Vm::install_jit_storage`
  keeps the storage it replaces until the `Vm` drops, and
  `luna_jit::jit::cache_clear` no longer drops code that compiled
  functions still call. On Windows Cranelift does not return the pages
  yet, so the code there is still kept.

---

## [3.2.0] — 2026-09-27

`string.dump` writes bytecode the stock PUC interpreter of each dialect
loads, the `luna` CLI follows `lua.c`, and a script or a loaded chunk can
no longer panic the VM. No breaking change: two renamed `Vm` methods keep
their old names as deprecated aliases.

### Added

- The `luna` CLI runs `LUA_INIT` as each dialect's `lua.c` does: a chunk
  named after the variable, or `@file`; from 5.2 on `LUA_INIT_5_x` is
  taken first and `-E` skips it; 5.1 runs it before reading the options.
- `-E` (5.2 on) now also makes the package library ignore `LUA_PATH` /
  `LUA_CPATH`, and sets the registry's `LUA_NOENV`, as in `lua.c`.
- `Vm::set_ignore_env` (`lua.c`'s `-E` for the libraries opened after
  it), `Vm::read_stdin_line` (`fgets` on stdin, through the io library's
  buffer) and `Vm::tostring_value` (`luaL_tolstring`) are public.

### Fixed

- On Windows, the error text of an `io` failure luna detects itself
  (`EINVAL`, `EBADF`, `ESPIPE`, `ENOMEM`) is the C runtime's, as PUC
  prints it (`Invalid argument`), instead of the unrelated Windows message
  for the same number.
- 5.1: a zero constant takes the sign of the first zero its function
  loaded, as PUC 5.1's constant table (keyed by value, where `0 == -0`)
  makes it, so `print(0, -1 * 0)` prints `0 0`. Constant folding also
  follows 5.1 and 5.2 more closely: parenthesized and negated operands fold,
  `%` and `^` fold in 5.1 and 5.2, and a division or modulo by zero is left
  to run time in every dialect.
- A stripped PUC chunk loaded into luna reported line 0 for every
  instruction instead of having no line information (`currentline` is now
  -1, and errors are placed at `?`, as in PUC).
- A numeric `for` loop whose hidden state was changed by `debug.setlocal`
  or by a crafted binary chunk panicked the host; it now raises
  `'for' state corrupted`. On 5.1/5.2 a number of the other
  representation is accepted there and the loop continues, as in PUC.
- A table constructor whose table was replaced the same way panicked; it
  now raises `attempt to index a <type> value`.
- The method JIT wrote the elements of a table constructor into the
  table's array part without checking its size; a smaller array part now
  takes the checked path. It also no longer compiles a constructor with a
  large start index, and no longer panics while compiling a function
  whose arithmetic result lands in a register that held a table, a math
  call on a table, or a `math.mininteger` loop step.
- The trace JIT passed a value that was not a table (a string's length,
  a field read through the string metatable, an index of a number with a
  metatable) to its table helpers, which read it as a table and could
  crash the process. Those operations are left to the interpreter.
- A generic `for` whose iterator is `pcall`, `xpcall` or `pairs` with a
  `__pairs` metamethod could corrupt the call stack inside a trace; the
  trace now leaves to the interpreter for it.
- The trace JIT ran a recursive call inline as the traced function
  whatever the call target was at run time, with the upvalues of the
  closure the trace was entered with: after the recursive local was
  reassigned, or when the target was another function or another closure
  of the same function, it computed the wrong result. An inlined call now
  leaves the trace unless its target is that closure.
- When the interpreter ran a side trace for a trace's exit, it restored
  the registers with the parent trace's summary of its exit types, which
  could turn a function or table in a register into an integer.
- A comparison of floats in a trace whose recorded branch was the
  negated one (`not (a < b)` compiled as `a >= b`) took the other branch
  when an operand was NaN.
- With `Vm::set_self_link_enabled(true)`, a self-linked trace whose
  recording held a loop edge or a call it does not inline (a crafted
  chunk) panicked the compiler or skipped the call; it is now not
  compiled.
- `docs/compatibility.md` said the bytecode verifier checks that the key
  of a field or global access is a string; it checks the constant index
  only.
- `debug.debug`: a command nested too deep for the parser (5.4+) now goes
  through the running message handler, as `load` does.
- Panics reachable from Lua scripts or loaded chunks, now Lua errors or
  the PUC result: `debug.getupvalue` / `debug.setupvalue` on a function
  loaded without upvalues; `debug.setupvalue` on a standard-library
  function (see Changed); `string.format` with a width past the machine
  word; `file:seek("cur", math.mininteger)` after a read; `bit32` shifts
  by `math.mininteger`; 5.1/5.2 `table.sort` on a range ending at
  `2^31-1`; `table.sort` with a huge `__len`; a 5.5 named vararg table
  whose `n` exceeds the stack (`stack overflow`); 5.1
  `debug.traceback` with a level past a C `int`; stores into a full
  table by `require`, `module`, `package.seeall`, `coroutine.wrap` and
  5.1/5.2 `table.insert` (`table overflow`); library-built strings
  longer than a string can hold (`string length overflow`); a binary
  chunk whose local names a register out of range or that nests
  functions deeper than 250 levels (refused on load); a named local of a
  crafted chunk aliasing a running call; a to-be-closed slot a crafted
  chunk registers twice (`'<close>' state corrupted`).

### Changed

- The `luna` REPL (`-i`, or no script with stdin a terminal) is each
  dialect's `lua.c` REPL: `_PROMPT` / `_PROMPT2` on stdout, a line tried
  as `return <line>;` first from 5.3 on and `=expr` through 5.4, results
  through the global `print`, errors with their traceback and without the
  program name, the version line first when stdin is a terminal, and a
  newline at the end of input. It used to print its own banner and
  prompts on stderr, render the results itself and report an error as
  `error: <message>`. Without the `repl-line-editor` feature it no longer
  writes `~/.luna_history`, as `lua.c` without readline keeps no history;
  the line editor still does.
- `string.dump` writes PUC bytecode of the running dialect (5.1, 5.2, 5.3,
  5.4 or 5.5), which that version's stock `lua` loads and runs;
  `string.dump(f, true)` strips debug information as PUC does. A function
  the dialect's instruction set cannot express raises `unable to dump given
  function`. `luna_core::vm::dump::dump` still writes luna's own format,
  and MacroLua's `string.dump` keeps it too (MacroLua has no PUC format).
- A binary chunk in the running dialect's own PUC format, which is what
  `string.dump` now produces, loads under the same switch as luna's own
  chunks (`Vm::set_bytecode_loading`, on by default); chunks of the other
  PUC versions still need `Vm::set_puc_bytecode_loading(true)`.
- `debug.setupvalue` no longer changes the upvalues of C functions; it
  returns nothing for them. Library functions keep state there that they
  rely on.
- `Vm::set_p16_self_link_enabled` / `Vm::p16_self_link_enabled` are now
  `Vm::set_self_link_enabled` / `Vm::self_link_enabled`. The old names
  still work and are deprecated.
- The published `luna-aot` description matches what it does: the trace
  JIT's machine code is part of the produced binary.

## [3.1.0] — 2026-09-25

A parity release. **No breaking change**: code written against 3.0 builds
unchanged. The public-API audit against 3.0.0 (rustdoc JSON of the five
library crates) finds no removed item, no changed signature, no field added
to an all-public struct and no variant added to an exhaustive enum; the
additions are listed below.

luna was compared against stock PUC 5.1.5, 5.2.4, 5.3.6, 5.4.9 and 5.5.1,
function by function, with 63 probe programs run on both macOS and Linux
(every standard-library function against missing, nil, wrongly typed and
numeric-string arguments; each library's surface; error messages; the
lexer; compiler limits; number formatting; what a program can see of the
collector). Every difference found was fixed, or is listed as deliberate in
`docs/compatibility.md`. The differential corpus grew from 514 to **805
fixtures**, each run both as source and as PUC bytecode compiled by that
version's `luac`.

### Added

- `Vm::call_value_with_handler` (PUC `lua_pcall` with a message handler),
  `Vm::traceback` (`luaL_traceback`), `Vm::metafield`
  (`luaL_getmetafield`), `Vm::load_file` (`luaL_loadfilex`) and
  `Vm::load_buffer` (`luaL_loadbufferx`).
- `vm::objname::getobjname_in`, the dialect-aware form of `getobjname`.
- A bytecode verifier, on by default: a binary chunk whose registers,
  constants, upvalues, jumps or nested functions point outside what the
  function has fails to load instead of crashing the process. PUC does not
  verify binary chunks.
- Smaller public items: `SyntaxError::render` / `SyntaxError::unpositioned`,
  `Lexer::line`, `numeric::strtod_str` (C `strtod`, used by 5.1), and the
  trace exit bits `EXIT_TAGS_INDEX_BIT` / `EXIT_KEEP_TFOR_VARS`.

### Changed

- **The `luna` CLI reports errors and sets its exit status as each
  dialect's `lua.c` does**: `lua: <message>` plus the message handler's
  traceback on stderr, exit status 1; non-string error objects, `-e`,
  `-l`, `-i`, `-v`, `-E`, `-W`, `--`, `-` and the usage message per
  version. luna's own flags are unchanged.
- **A NaN is spelled the way the platform's `printf` spells it**, as PUC
  does: `nan` on macOS, `-nan` for a negative NaN on Linux (0/0 is one on
  x86), `-nan(ind)` on Windows.
- Binary-chunk load errors use each dialect's `lundump.c` wording and
  chunk-name prefix.
- The 5.4 ground truth moved from PUC 5.4.8 to 5.4.9.

### Fixed

Standard library (each against that dialect's own `l*lib.c`):

- Argument errors: the function is named as the caller named it (5.1
  `'?'`, 5.2's global-table walk, 5.5 `bad extra argument`), with each
  dialect's number conversions and `no value` / `__name` handling.
- `collectgarbage`: options, results and parameter encodings per dialect.
  Tables are finalized only from 5.2 on.
- `string.format` rebuilt on a model of C `printf`; `string`, `string.pack`
  and `utf8` per dialect (utf8 is absent from 5.1/5.2); string arithmetic
  and coercions per dialect.
- `math`, `table` and `bit32` follow `lmathlib`, `ltablib` and `lbitlib`.
- `io`, `os`, `package` (real searchers) and `coroutine` per dialect.
  5.1's `file:seek` returns a number that prints as a double.
- `load`, `loadfile`, `tonumber` with a base, `tostring` via
  `luaL_tolstring`, `getfenv` / `setfenv` levels and `newproxy` (5.1).

Language and VM:

- Syntax errors name the token PUC names. `goto` and labels are checked
  while parsing. Numeric `for` loops are checked and counted per dialect;
  a NaN in a 5.4/5.5 float loop runs the body once, as there.
- Register limits per dialect (5.1/5.2 fail at 250, 5.3/5.4 at 255, 5.5
  above 255), 5.1's 60-upvalue limit (a hidden environment slot took one),
  and the right error for a `return` of 255 or more values.
- Metamethods: `__le` from `__lt` in 5.4, the same `__eq` on both sides in
  5.2, one `__call` hop before 5.4, `__index` chains bounded at 100 in
  5.1/5.2, `__name` ignored before 5.3. Calling a metamethod that is not
  a function names it (`(metamethod 'sub')`) only from 5.4 on.
- `x - 0` with a literal integer zero runs as `x + 0` in 5.4/5.5, as PUC
  compiles it (`ADDI`), so `-0.0 - 0` is `0.0` there; a float result of
  zero or NaN is no longer constant folded from 5.3 on (found by fuzzing).
- `x ^ 2` squares by multiplying in 5.4/5.5, as their `luai_numpow` does,
  in the interpreter and the JIT; the C library's `pow` can differ in the
  last bit (found by fuzzing).
- Multiple assignment stores from the last target to the first. 5.4+
  `<const>` locals with constant values are compile-time constants.
- An `xpcall` message handler that raises runs again where it raised, as
  PUC's `luaG_errormsg` does; `xpcall(error, error)` gives `error in
  error handling`; 5.5's `<no error object>` is not handled twice.
  `pcall` / `xpcall` called from a library function (`string.gsub(s, p,
  pcall)`) return their results.
- The debug library walks the stack as PUC's `CallInfo` chain does:
  levels, names, tracebacks, hooks, `getinfo` options.

Binary chunks (found by fuzzing):

- A count read from a luna dump or from AOT trace metadata sized an
  allocation before anything checked it, so a corrupt count aborted the
  process out of memory. Counts the rest of the input cannot hold are now
  refused as a truncation.
- A PUC 5.1/5.2 operand wider than luna's 8-bit field (their B and C have
  9 bits) was encoded into the neighbouring field in release builds,
  yielding a different instruction. The translators now refuse it.

PUC bytecode:

- A chunk from the running dialect's own PUC version reaches the
  translator. Four corpus programs compiled by PUC's `luac` crashed the
  process with SIGBUS when loaded (the translator mis-encoded jump
  offsets); they load and run now.

JIT:

- Reading a table with a NaN, infinite or out-of-range float key in
  method-JIT code killed the process with SIGILL (since 1.0).
- Method-JIT table reads check the type of what they read: a missing key
  came back as integer 0.
- A trace whose table helper declined an operation (a metatable) restarted
  the iteration and ran its earlier stores twice.
- About fifteen further trace and method-JIT defects found by running the
  whole corpus with the JIT forced on, among them a trace loop that never
  ended and a failing trace recompiled on every call.

### Performance

Same-runner perf-gate against 3.0.0 (x86_64): `sliding_window_500`
0.57×, `token_bucket_1k` 0.95×, `method_dispatch_5k` 0.96×,
`string_ops_2k` 1.00×, `dict_5k_lookup` 1.01× (earlier runs on other
runners measured the last two at 0.88× and 0.87×).

A trace no longer carries registers its loop only writes around the loop.
Keeping the in-memory register state current asked the SSA builder for
every register at each step, which gave the loop head a parameter for each
one; six of token_bucket's ten were never read but stayed live across
every helper call. Removing unused block parameters after lowering took
that trace's compile from 663 to 597 µs and the cell from 947 to 876 µs
(3.0.0: 879; aarch64, median of 400).

### Correction to 3.0.0

3.0.0 closed its fuzzing criterion on a weekly schedule plus a
`crash-check` job that fails the run when a target uploads a crash. That
job could never fail: the upload looked in the wrong directory, so nothing
was ever uploaded, while three targets (`fuzz_aot_meta`,
`fuzz_dump_reader`, `fuzz_diff_puc`) failed every week from at least
2026-07-20. The upload path is fixed, `crash-check` now also fails on the
fuzz jobs' own results, and the findings are fixed above.

### Documentation

- `docs/compatibility.md` lists the deliberate differences from PUC, and
  no longer calls the C API a drop-in for PUC's `lua.h`: several names it
  covers are macros there that expand to functions luna does not export.
- `docs/performance.md` corrected (a claimed benchmark track never shipped).

## [3.0.0] — 2026-08-14

The v2.x maturity arc's destination. **No breaking change** — the major
bump marks the maturity gate, not an API break. `luna_core`'s public
surface is identical to 2.18.0 (788 items, zero removals, zero
additions), and code building against 2.x builds against 3.0 unchanged.

### What 3.0 asserts

The arc opened (2026-06-28) with a list of ten things that had to be
true before luna could be called mature. Where each landed:

| | Criterion | How it is evidenced today |
|---|---|---|
| 1 | No known UAF / heap corruption | ASAN nightly over the full PUC official suite across 5 dialects, 48 consecutive greens; Miri on the library plus a regression subset; zero open known-bugs |
| 2 | Differential parity with PUC 5.1–5.5 | 514 private-corpus fixtures byte-identical to stock PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.8 / 5.5.1, zero skips, gated on every push; the official suite passes end-to-end on all five dialects |
| 3 | 24h-equivalent soak clean | Four capped runs, ~23h cumulative, second-half RSS drift under 1% with no vm_mem accumulation; six later samples corroborate |
| 4 | Cross-allocator clean | glibc + jemalloc + mimalloc + Apple-malloc, nightly |
| 5 | Cross-platform matrix green per push | ubuntu / macos / windows / ubuntu-arm × stable, plus wasm32 targets, perf-gate, feature matrix and the zero-dependency contract |
| 6 | Perf floor closed or ceiling explicit | v2.9 established a structural ceiling with a decomposition record |
| 7 | API stability | Enforced per release by a public-surface audit: a minor bump must show an empty removal list |
| 8 | *(removed from the gate)* | Adoption is a product outcome, not an engineering one; tracked as an observed fact rather than a precondition |
| 9 | Documentation complete | Every public item documented, `deny(missing_docs)` |
| 10 | Fuzz corpus established | 7 targets on a weekly schedule, with a gate that fails the run if any target produces a crash artifact |

### Changed since 2.18.0

Nothing in the runtime. This release is the arc's closing record plus CI
and tooling repair:

- **`luna-aot` cross-compilation errors now give correct advice.** The
  "CARGO_MANIFEST_DIR not set" message suggested setting
  `LUNA_AOT_RUNTIME_HELPERS_STATICLIB`, which is honoured for host
  builds only — following that hint on a cross target produced the same
  error with nothing left to try. Cross builds are now told what
  actually works (run luna-aot from inside its workspace) and why the
  override does not apply. **The underlying limitation stands and is
  worth knowing: a standalone `luna-aot` binary cannot cross-compile**,
  because a cross target always builds its own runtime-helpers staticlib
  and that needs the workspace.
- Two CI jobs that had never once completed — Alpine musl E2E and Wine
  PE execution — were repaired and are now blocking. Between them they
  had been masking a real product path (standalone cross-compilation)
  and four CI defects.
- Toolchain-lint drift now gets previewed on beta and nightly ahead of
  each stable release; the LuaJIT differential reference is asserted
  rather than inherited from the runner image; the site's links are
  link-checked; dependency major-version lag is measurable.

### For embedders

- **No migration needed.** Public API unchanged from 2.18.0.
- luna declares **no MSRV** (since 2.17.0). The promise is "builds on
  current stable", which the four-platform matrix tests on every push.
  Informationally, the tree currently needs rustc 1.88+.
- The dialect-sensitive behaviour corrected in 2.18.0 (5.1/5.2 type-error
  wording, table-library surface and raw element access, `__len` on 5.1)
  is unchanged here — see that entry if you are coming from 2.17 or
  earlier.

## [2.18.0] — 2026-08-14

Upstream reference moved to **PUC Lua 5.5.1** (released 2026-07-24), and
six real defects were found and fixed in the process. One is a denial of
service; three change behaviour on the 5.1/5.2 dialects.

### Fixed
- **`string.rep` hung the VM when the piece was empty (DoS).**
  `string.rep("", math.maxinteger, "")` looped `n` times copying zero
  bytes; the size guard cannot catch it because `0 * n` never overflows.
  A single expression could hang any embedder running untrusted Lua. luna
  matched PUC 5.5.0 here, which has the same bug; PUC fixed it in 5.5.1.
- **The table library's surface ignored the dialect.** luna registered
  the union of every version's functions, so 5.1 saw
  `table.create`/`move`/`pack`/`unpack` and 5.2 saw `create`/`move`,
  while 5.1 was missing `setn` and 5.2 was missing `maxn`. Code written
  against an older dialect ran here and broke on real PUC. Registration
  is now ordered by version, and `table.setn` exists on 5.1 to raise
  "'setn' is obsolete" exactly as PUC does.
- **Type errors were worded wrongly on 5.1/5.2.** PUC ≤5.2 names the
  operand first — `attempt to call field 'f' (a nil value)`; 5.3 flipped
  to type-first. luna emitted the 5.3+ form everywhere, so every type
  error under 5.1/5.2 (calls, indexes, arithmetic) had the wrong shape.
  ≤5.2 also has no metamethod operand names, so those collapse to the
  bare message.
- **Table-argument checking and element access now follow the dialect.**
  PUC changed this twice: 5.1/5.2 demand a real table and read elements
  raw (`lua_rawgeti`); 5.3+ accept anything with the needed metamethods
  (`checktab`) and read through `__index` (`lua_geti`); 5.5 additionally
  exempts strings from the `__len` requirement and is the first version
  to type-check `table.unpack` at all. luna combined a hard check with
  metamethod access, matching no dialect. Visible effect, concatenating a
  proxy whose contents sit behind `__index`: 5.1 gives `""`, 5.2 raises
  `invalid value (nil) at index 1`, 5.3+ reads through.
- **`__len` no longer applies to tables on 5.1.** PUC 5.1's `__len` is a
  userdata-only metamethod, so `#setmetatable({}, {__len = f})` is `0`
  there and `7` on 5.2+. luna called the metamethod on every dialect.
  This governs every `#` on 5.1, not just the table library.
- **Errors naming a value read from a named-vararg table lost the field
  name.** `t.k` inside `function f(...t)` compiles to a dedicated opcode
  rather than `GETFIELD`, and the operand-naming walk did not recognise
  it: `attempt to perform arithmetic on a nil value` instead of
  `… (field 'xx')`.
- **Locals left scope before their block's `CLOSE` ran.** A `__close`
  handler calling `debug.getlocal` on the enclosing frame saw
  `(temporary)` rather than the variable name, because the block's
  `OP_CLOSE` was emitted after each local's scope had been closed off.
  Most visible in a `repeat` body, where the exit path was affected and
  the loop-back path was not.

### Changed
- Differential basis is now PUC **5.5.1** throughout: the corpus builds
  against the 5.5.1 tarball, `tests/official/` carries the 5.5.1 suite,
  the fuzz workflow's reference interpreter is 5.5.1, and CI asserts the
  5.5 interpreter is exactly 5.5.1 rather than accepting whatever the
  runner image ships.
- Private differential corpus 500 → **514** fixtures, byte-equal against
  stock PUC 5.1.5 / 5.2.4 / 5.3.6 / 5.4.8 / 5.5.1 with zero skips.

### Note on how these were found
Re-running the corpus against 5.5.1 produced **zero** divergences. That
was not reassurance — it meant the corpus could not reach what upstream
had changed. Reading the 5.5.0→5.5.1 source diff and probing the
behaviour it touched, three ways (5.5.0 / 5.5.1 / luna), is what surfaced
all six defects. A 210-cell dialect matrix (7 argument shapes × 6 table
operations × 5 dialects) went from 27 divergent cells to 0.

## [2.17.0] — 2026-08-14

Maintenance release. No runtime behaviour changes; the VM, its dialect
semantics, and the C ABI are untouched. Everything here is metadata,
tooling, or disclosure — but two items materially affect embedders, so
read Changed before upgrading.

### Removed
- **luna no longer declares an MSRV.** `rust-version` is gone from the
  workspace and from all nine crate manifests, and the `msrv` CI
  workflow is deleted.

  It had said `1.86` since v1.1.0 and that was **false the entire time**:
  the tree has used edition-2024 let-chains since `2bbda24`
  (2026-06-23), and let-chains need rustc **1.88**. Measured: 1.87 fails
  with `E0658: 'let' expressions in this position are unstable`. Eleven
  releases shipped a compatibility promise the code did not keep.

  Nobody caught it because the `msrv` workflow triggered on a `main`
  branch this repository does not have — it never executed once. An
  unverified pin is worse than no pin: it misleads people who trust it.

  Rather than re-pin and carry the upkeep, luna makes no MSRV promise.
  What it is willing to guarantee is "builds on current stable", which
  is what the CI matrix actually tests on every push across ubuntu /
  macos / windows / ubuntu-arm.

  **Informational, not a contract** (may move without a version bump):
  as of 2.17.0 the tree needs **rustc 1.88+**. With no declared floor,
  an older toolchain surfaces as a compiler error (`E0658`) rather than
  a cargo manifest error — if you see that, upgrade rustc rather than
  suspecting your own code.

### Fixed
- **Disclosure — `numeric::num_to_string_for` changed signature in
  v2.14.0 and this was not announced.** The parameter went from
  `legacy_float: bool` to `fmt: FloatFmt`:

  ```rust
  // <= 2.13.0
  pub fn num_to_string_for(n: Num, legacy_float: bool) -> String
  // >= 2.14.0
  pub fn num_to_string_for(n: Num, fmt: FloatFmt) -> String
  ```

  `luna_core::numeric` is a `pub mod`, so this is inside the stability
  contract stated at the top of this file, and it shipped in a *minor*
  release — a semver violation on our part. The v2.14.0 entry mentioned
  the new `numeric::FloatFmt` type under **Fixed** but never said the
  function's signature had changed, and carried no `BREAKING` marker.

  No compatibility shim is being added: a `bool` cannot express the three
  per-dialect float formats v2.14 had to distinguish, so the enum is the
  correct shape and restoring the old overload would permanently carry an
  API that cannot say what the function does.

  Migration — pass the variant instead of the bool:

  | Old call | New call | Dialects |
  |---|---|---|
  | `num_to_string_for(n, true)` | `num_to_string_for(n, FloatFmt::Legacy14)` | 5.1, 5.2 (`%.14g`, no `.0`) |
  | `num_to_string_for(n, false)` | `num_to_string_for(n, FloatFmt::TwoStage55)` | 5.5 (`%.15g` → round-trip → `%.17g`) |
  | *(not expressible)* | `num_to_string_for(n, FloatFmt::G14)` | 5.3, 5.4 (`%.14g` + `.0`) |

  The third row is the reason for the change: v2.13 applied the 5.5
  scheme to 5.3/5.4 as well, which was one of the divergences v2.14
  fixed. `FloatFmt` has no constructor from a `LuaVersion` — pick the
  variant directly, as the VM itself does.

  Found by the public-surface audit now run every release
  (786 → 788 public items over v2.13.0→HEAD; this was the only
  removal/signature change).

### Internal
- Cleared 25 `clippy` 0.1.97 findings (22 `collapsible_if` → edition-2024
  let-chains, plus `unnecessary_parens`, a redundant `&`, and a
  `match` → `?`). CI had been red for 34 days. The trigger was an
  unreleased commit raising the MSRV declaration to `1.97`: clippy's
  let-chain suggestion is MSRV-gated, so a higher declared floor
  unlocked it. With no MSRV declared at all, clippy assumes current
  stable and the tree is clean — verified.
- `cargo-deny` now runs with `--all-features`. It had been resolving only
  the default feature set, leaving every crate behind luna-tools' opt-in
  `flame-graph`/`mcode-disasm` features unscanned; restoring coverage
  surfaced a yanked `spin 0.10.0` (bumped to 0.10.1).
- `soak-weekly` now finishes inside the runner cap instead of being
  killed every week — 11 consecutive `cancelled` runs made the job
  useless as a signal even though its artifacts were valid. Six
  backlogged samples analysed: 6/6 clean, second-half RSS drift
  0.414%–0.899%, all under the 1% bar.
- `actions/checkout` and `actions/upload-artifact` upgraded v4 → v7.

## [2.16.0] — 2026-07-06

### Changed
- **v3.0 differential-parity acceptance narrowed to two legs.** The
  official-suite *byte-diff* leg is dropped from the acceptance set;
  parity is now proven by (a) the 500-fixture private corpus being
  byte-identical to PUC across all five dialects, gated on every push,
  and (b) the official suite's assert-count instrumentation running
  nightly. Measured divergence on the byte-diff harness was 24% (against
  a 5% estimate), split between files where the harness's body-wrap
  breaks PUC's own scope semantics and genuine numeric-formatting
  internals; closing it would have needed a per-dialect allowlist roughly
  twice the size the design permitted.

### Added
- Opt-in official-suite byte-diff harness (`LUNA_OFFICIAL_BYTE_DIFF=1`,
  with `PUC_LUA_5X` binaries) as a local diagnostic surface for
  investigating per-file divergence. Not a CI gate.

## [2.15.0] — 2026-07-06

### Added
- **Differential corpus 400 → 500 fixtures**, every one byte-equal to
  its stock PUC interpreter with zero skips: 15 compiler-shape and 25
  utf8 fixtures for 5.5, a 14-fixture 5.4 batch (to-be-closed variables,
  `<const>`, `coroutine.close`, generational GC, `__pairs`), and 43
  fixtures across 5.1/5.2/5.3. Every dialect now carries at least 25
  fixtures (5.1=25, 5.2=25, 5.3=25, 5.4=25, 5.5=400).
- ASAN nightly now runs the full official suite across all five dialects,
  measured at 5m34s under Docker and 2–3 min on hosted runners.

### Changed
- **Miri's acceptance leg narrowed to the library plus a representative
  integration subset.** Full `official_run` under Miri measured at 30 min
  – 2.5 h per nightly; the marginal provenance/UB coverage beyond the
  `--lib` gate is small against luna's bounded unsafe surface (~2000 LOC),
  and the combinatorial surface it would add is what the ASAN gate above
  already covers.

## [2.14.0] — 2026-07-05

### Added
- **Multi-dialect differential harness** — the luna-vs-PUC diff
  corpus now runs per dialect: `tests/diff_puc/5.1/ … 5.5/`
  subtrees execute against stock PUC 5.1.5 / 5.2.4 / 5.3.6 /
  5.4.8 / 5.5.0 interpreters (built from source in CI). Ground
  truth is each version's DEFAULT `make` build, compat flags
  included.
- **Error-channel comparison** — `*_err.lua` fixtures pin the
  error path: both interpreters must fail at top level (non-zero
  exit ⇔ eval Err) with matching normalized error text.
- **Corpus 250 → 400** — dialect seed batches (10+ per legacy
  dialect), io/os deterministic batch, 33-fixture error-channel
  batch, coroutine deep matrix, string.pack format matrix,
  metamethod/core-semantics batch.
- `Vm::error_display` — renders an error value the way PUC's
  standalone message handler does: numbers stringify and
  non-string objects get their `__tostring` called before
  collapsing to the `(error object is a … value)` tag. The
  non-executing `Vm::error_text` is unchanged.

### Fixed
23 real divergences against stock PUC interpreters, including:
- **Per-dialect float formatting** — ≤5.2 prints `%.14g` with no
  `.0` suffix, 5.3/5.4 `%.14g` + `.0`, 5.5 the two-stage
  `%.15g`→`%.17g` (new `numeric::FloatFmt`; v2.13 had applied the
  5.5 scheme to every dialect).
- **Per-dialect arithmetic error wording** — 5.4+ report
  string-involved arithmetic faults as `attempt to add a 'string'
  with a 'number'` (lstrlib's string-metatable handlers); ≤5.3
  keep the aggregate wording.
- **`error(nil)` substitution timing** — `<no error object>` is
  substituted only at the catch point (after a message handler
  ran), so xpcall handlers and the top level see the raw nil,
  matching `luaG_errormsg`.
- Dialect-gated stdlib surface: `loadstring` exists on 5.2 and
  `bit32` on 5.3 (stock `LUA_COMPAT_*` builds); `math.type` /
  `tointeger` / `ult` are 5.3+; 5.1 `xpcall` does not forward
  extra arguments; 5.4/5.5 boot in generational GC mode.
- `os.date` implements the C99 strftime specs PUC inherits
  (`%u %e %C %D %T %F`) and the ISO 8601 week date (`%G`/`%V`).
- luaL-conformant wording/returns: `file:setvbuf`/`file:flush`
  return `true`; `string.rep`/`gsub`/`format` integer arguments
  follow `luaL_checkinteger` (numeric strings convert, failures
  say `bad argument #N … (number expected, got T)`);
  pcall/xpcall-invoked natives qualify their names via
  `package.loaded` (`'coroutine.resume'`, not `'resume'`);
  `table.concat` element errors carry the type name;
  `invalid key to 'next'` has no position prefix.

### Changed
- **CI perf-gate is now a same-runner comparison** — the fixed-ns
  baseline was invalid on heterogeneous hosted runners (identical
  code measured 0.505x–1.087x across runs). The gate benches a
  pinned reference commit on the same runner immediately before
  HEAD and diffs the two. Same-runner verdict for this release:
  every cell within 0.98x–1.03x of v2.13.0.

## [2.13.0] — 2026-07-04

### Fixed
- **Stacked Borrows UB in Table/LuaClosure inline storage** — the
  cached self-referential pointers (`array_ptr` / `upvals_ptr`,
  from the P11-S5d inline-array and closure-inline optimizations)
  were invalidated by every `&mut self` function-entry retag;
  accesses through them were undefined behavior with real
  miscompilation risk under rustc's noalias annotations. Inline
  storage now lives in `UnsafeCell` and accessors derive the base
  pointer fresh at each use; the Miri nightly lane passes for the
  first time since it landed.
- **UAF-C closed** — the Windows gc.lua `STATUS_ACCESS_VIOLATION`
  (gated since v2.4, perma-gated v2.8 as "repro infeasible") was
  root-caused to two platform-independent GC bugs and fixed:
  an explicit `collectgarbage()` collected with a stale stack-root
  cursor and swept its caller's live register values (fix: PUC
  C-call discipline — entering a native raises the cursor to the
  argument top), and weak-table tombstone keys (`t[k] = nil`)
  escaped the clear-key sweep and were freed while hash-chain walks
  still compared them (fix: PUC `clearbykeys`-style unconditional
  key demotion on empty entries). The Windows CI gate on
  gc.lua/gengc.lua/tracegc.lua is physically removed; validated by
  25× Linux ASAN stress and 6 consecutive 50-iteration Windows
  stress runs on native-heap and poison-allocator lanes.
- `coroutine.resume` status errors ("cannot resume dead coroutine"
  etc.) no longer carry a position prefix, matching PUC
  `resume_error`.
- `debug.getupvalue` / `debug.setupvalue` return zero values (not
  nil) for out-of-range indices, matching PUC `db_getupvalue`.
- Float `tostring` now matches PUC 5.5 byte-for-byte: two-stage
  `%.15g` → round-trip check → `%.17g` (lobject.c
  `tostringbuffFloat`), replacing Rust's shortest-round-trip
  spelling (`math.pi` now prints `3.1415926535897931`).
- `error()` invoked directly by a C function (e.g.
  `pcall(pcall, error, "msg")`) no longer carries a position
  prefix — `luaL_where` now counts continuation activations as C
  frames.

### Added
- `gc-verify` feature (zero-dep, diagnostic): luna's
  `lua_checkmemory` analogue — post-sweep dangling-reference walk,
  atomic-phase tricolor invariant check, post-collect rooted-stack
  liveness audit, and freed-pointer read-time probes.
- Differential corpus 150 → **250** fixtures, all byte-equal
  against PUC 5.5 with zero skips (metamethod full sweep, coroutine
  edges, string.format matrix, pattern engine sweep, float-spelling
  matrix, and more). Pinned 5.5 semantics: `__pairs` restored,
  generic-for control variable is `<const>`.
- `uafc-windows-stress.yml` dispatch workflow: procdump-hosted
  gc.lua stress on windows-latest with cdb stack capture.
- luna-soak writes its JSON report incrementally after every
  sample, so a run killed by the GHA 6-hour job cap still uploads
  a complete partial report.

---

## [2.12.0] — 2026-07-02

- Differential corpus 100 → 150 fixtures, all byte-equal PUC 5.5
  with **zero skips** — the harness now fails loudly when a fixture
  errors on PUC itself (previously silently skipped; 5 never-diffed
  broken fixtures repaired).
- `math.modf` returns its integral part as an integer (PUC
  5.4/5.5 semantics); arithmetic-on-string error wording documented
  as a deliberate cross-dialect design (PUC 5.4 wording retained).
- soak pipeline zero-data root cause fixed (see Unreleased:
  incremental report write) and 4 latent CI bugs repaired — the
  diff-puc workflow went green for the first time since landing.
- `docs/embedder-recruitment.md` — public call for luna's second
  production embedder.

## [2.11.0] — 2026-07-02

- Differential corpus 45 → 100 fixtures.
- `#![deny(missing_docs)]` across the public API (was warn).

## [2.10.0] — 2026-07-02

- Differential corpus 5 → 45 fixtures across arithmetic, strings,
  tables, closures, coroutines, metamethods, and stdlib edges.
- Docstring audit closed (v3.0 acceptance #9).

## [2.9.0] — 2026-07-01

- Perf positioning finalized: the LuaJIT 1.18× charter floor is
  documented as a structural ceiling (trace-JIT architecture
  class); luna's lane is the correctness-first Rust-native Lua VM
  competitive with the PUC 5.4/5.5 interpreter. Full decomposition
  evidence in-tree.

## [2.8.0] — 2026-07-01

- Public API stability contract documented
  (`docs/embedding.md` §13); the 6-month API-stability clock for
  v3.0 acceptance #7 starts at v2.7.0.
- Windows gc.lua UAF gated + documented as a known limitation
  (superseded in v2.13 — root-caused and fixed).
- Windows arm64 AOT deferred (vendored cranelift PE/COFF Aarch64
  GOT relocations unimplemented upstream).

## [2.7.0] — 2026-07-01

- Per-PR perf-gate made required (redis_lua_shape 5-cell bench vs
  committed baseline, 1.05× regression threshold).
- Embedder API audit; cross-platform CI matrix extended with
  Linux arm64 (ubuntu-24.04-arm).

## [2.6.0] — 2026-06-30

- Per-PR perf-gate infrastructure (advisory) + nightly
  luna-vs-LuaJIT differential lane + Boltzmann program-generator
  grammar extension.
- Frame-pop slot-clear chain completed (P1B-2E minimal
  tightening) — later found to be one origin of UAF-C (see
  Unreleased).

## [2.5.0] — 2026-06-30

- Slot-clear discipline at every Lua-side frame-pop site
  (`finish_results`, `Op::TailCall` collapse, pcall unwind),
  mirroring PUC's `L->top` hygiene.

## [2.4.0] — 2026-06-29

- Soak-test harness (`luna-soak`: RSS + `Vm::memory_used`
  sampling with JSON reports) + nightly 1h / weekly 24h-capped
  soak workflows.
- cargo-fuzz corpus seeding + nightly fuzz CI hardening.

## [2.3.0] — 2026-06-29

- UAF-A fully closed (sort.lua AA load+collectgarbage SIGSEGV) —
  `finish_results` slot-clear root fix; every v2.2.0 CI byte-strip
  and env-skip gate physically removed.
- ASAN and Miri nightly CI lanes.

## [2.2.0] — 2026-06-28

- UAF-B closed (toomanyidx memory-cap SIGSEGV under glibc).
- Test-infrastructure foundation per the v2.x → v3.0 maturity arc
  charter: ASAN docker environment, differential-vs-PUC harness
  (first 5 fixtures), cross-allocator test matrix groundwork.

---

## [2.1.0] — 2026-06-28 (the v2.0 mega sprint)

> Shipped 2026-06-28 to crates.io as 7 crates (luna-core /
> luna-jit-derive / luna-jit-helpers / luna-jit-llvm / luna-jit /
> luna-runtime-helpers / luna-aot), 235 commits since v1.3.0.
> The sprint log below is preserved as shipped-scope record —
> 14 tracks (J/R/PI/AO/MM/DS/CV/DO/PU/AT/TL/BM/CB/SQ) per
> the v2.0 charter, collapsed from what would have been
> v1.4–v1.8 under the `nodefer` directive.

### Phase 0 — 13 parallel audits (2026-06-25)

- Tracks J / R / PI / AO / MM / DS / CV / DO / PU / AT / TL / BM /
  CB each spawned a read-only audit agent. 13 RFCs landed; PI's
  full 26 KB body preserved;
  12 others' summaries (100–150 word + top-3 risks each) inlined as
  truth-of-record in the plan state.
- v2.0 Track SQ (textbook-grade source quality) added as Track 14
  per user request mid-Phase-0, sequenced LAST. Audit at
  the source-quality audit (45 KB / 821 lines).

### Phase 1 — Correctness backfill (CB)

- **CB-pre1** + **CB-pre2** verify-and-archive: pre-existing v1.0
  debug-mode SIGTRAP + `debug_upvalue_order_and_id` flakiness both
  cleared by the v1.3 fix chain (`fae0f9c` / `e5db587` / `f8afd64`);
  bug docs moved to the fixed pile.
- **CB-or** assert-counter wrapper at
  `crates/luna-core/tests/official_run.rs` + per-PUC-file coverage
  report. 140 PUC files
  exercised, 2.4M asserts reached, 2.36M passing, 101/140 files at
  ≥80% hit rate, 27/29 below-80% are PUC-internal early-return
  shims, 2 wrapper carve-outs (`errors.lua` / `db.lua` × 5 dialects).
- **CB-edge** 13 spot tests pinned: 5 GC finalizer
  (`cb_edge_gc_finalizer.rs` — recursive collect / weak-key+finalizer
  / userdata-as-key / 1000-proxy stress / error-in-`__gc`) + 3
  coroutine + hook (`cb_edge_coroutine_hook.rs`) + 6 compiler
  stress (`cb_edge_compiler_stress.rs` — 2000-stmt fn body /
  150-deep + 250-deep nesting cap / 60-deep paren / 100 upvals /
  50-arg vararg forward).
- **CB-edge real bug surfaced + fixed**: `Vm::set_hook` predicate
  `target.is_none()` arm was missing — `debug.sethook(…)` called
  from inside a coroutine body silently dropped. Root cause at
  `crates/luna-core/src/vm/exec.rs:2151-2179`; regression test +
  sibling test landed; the known-bug doc moved to the fixed pile.

### Phase 2 — Coverage + fuzz infrastructure (CV-infra)

- New workspace-excluded `crates/luna-fuzz/` crate with 4
  `fuzz_target!` harnesses: parser, dump_reader, vm_dispatch,
  aot_meta. Nightly toolchain pinned via crate-scoped
  `rust-toolchain.toml`.
- `.github/workflows/coverage.yml` — `cargo llvm-cov` workspace
  vs committed JSON baseline; fails PR on > 2pp regression in any
  first-party crate.
- `.github/workflows/fuzz.yml` — 5-min PR smoke (non-blocking) +
  60-min weekly cron per target.
- luna-core 0-third-party-dep contract intact: `libfuzzer-sys` lives
  only in the excluded fuzz crate.

### Phase 3 — Docs CI gate (DO-CI)

- `.github/workflows/docs.yml` — `cargo doc -D warnings` +
  `cargo test --doc` + `lychee` link check.
- 1 pre-existing intra-doc warning in `crates/luna-aot/src/embed.rs`
  fixed; 1 stale anchor in `docs/threading.md` fixed.
- `.lycheeignore` configured for pre-publish `docs.rs/luna-*`
  redirects.

### Phase 4 — PUC bytecode polish punts collapsed (PU)

PU audit identified 24 polish punts across 5.1/5.2/5.3/5.5 (5.4
already punt-free at v1.3 ship). Wave 1 extracted three shared
helpers (`lower_k_via_tmp` / `lower_i_imm` / `scan_tforprep_sites`)
to `crates/luna-core/src/vm/dump/puc/mod.rs`. Waves 2-4 collapsed
the punts dialect-by-dialect:

- **5.1**: PC remap upgraded to bidirectional (modeled on `puc_54.rs`),
  then 7/7 punts collapsed — SETLIST C=0 / arith RK-on-B (12 ops via
  `lower_k_via_tmp`) / EQ/LT/LE RK / LOADBOOL true+skip (via
  `LoadTrue + Jmp+1` pair through PC remap) / fb2int NEWTABLE hint /
  TFORLOOP N-way split (lower to `TForCall + TForLoop` via new
  `JumpKind::TForLoop` fixup; `A` direct = iter_base, differs from
  5.3) / LUAI_COMPAT_VARARG (runtime cold-path at `exec.rs:4200`
  already in v1.3; Wave 4 added the E2E test).
- **5.2**: 9 cases across 3 categories collapsed — arith K-on-LHS /
  arith K-on-both (inline pair) / EQ/LT/LE K (inline, since luna's
  `Op::Eq/Lt/Le` `k` bit is sense not constant flag) / GETTABUP
  register key (inline `GetUpval + GetTable` pair). 5.2 now
  punt-free.
- **5.3**: All 4 punts collapsed — generic-for (`TFORCALL + TFORLOOP`
  with `A = iter_base + 2 → iter_base` conversion since 5.3 lacks
  `OP_TFORPREP` but no TBC machinery either) / arith RK-on-B (12 ops
  via PC remap + helper) / LOADBOOL true+skip (Jmp pair through
  Fixup channel) / CONCAT B != A (Move-then-Concat pair with
  overlap-safe direction). 5.3 now punt-free except `OP_JMP A!=0`
  close-upvals (out of original 4-punt audit scope).
- **5.5**: 8/8 I-imm ops collapsed — ADDI / SHRI via `lower_i_imm`;
  SHLI / EQI / LTI / LEI / GTI / GEI inline (different shapes than
  `lower_i_imm`'s arith template). 5.5 now punt-free.

luna now loads `.luac` files from PUC 5.1 through 5.5 (and MacroLua)
without silent miscompile across the previously-punted opcode shapes.

### Phase 5 — Measurement-first baselines + documentation floor

#### Memory (MM)

- `dhat` dev-dep + `crates/luna-core/benches/mem_baseline.rs`
  exercising 5 workloads (cold_start / repl_idle / host_roots_churn /
  alloc_collect / userdata_lifecycle). Baseline snapshots at
  a local memory baseline.
- luna-core prod 0-dep contract preserved via `--edges normal` flag
  on `cargo tree`.
- Surprising finding: `TraceRecord::start` allocates ~557 KB across
  68 sites in `userdata_lifecycle` — confirms audit R2 (MM #5
  TraceRecord shrink blocked on Track R IR shape).
- Newly-surfaced attack candidate: `Vm::gc_roots` snapshot vec
  reallocs every GC (198 KB / 218 allocs in `alloc_collect`) —
  reusable.

#### Disk + binary size (DS)

- Local baselines covering per-crate
  package sizes, AOT output binary sizes (3 representative scripts ×
  3 build profiles), Mach-O section breakdown, runtime-helpers
  staticlib/rlib. Zero material drift from v1.3 audit values.
- 11 budget proposals with feasibility tags. AOT slim-profile output
  ≤ 3.7 MiB stripped tagged HIGH effort (requires both
  `panic="abort"` and Cranelift `all-arch` opt-out, both gated
  breaking changes).

#### Coverage (CV) gap fill

- 38 new tests + 1 new CI job (`send-feature`) across the audit's
  top-5 coverage gaps: async_drive (5 tests) / pattern engine (12
  tests) / aot_meta walker error paths (10 tests) / luna-jit-derive
  direct unit tests (11 tests, via inline `#[cfg(test)] mod`
  reaching private fns without `pub(crate)` hatches) / send_vm
  feature-matrix CI (8 SendVm tests already existed behind
  `#[cfg(feature="send")]`, no CI job exercised them).
- Zero real bugs surfaced.

#### Docs (DO) — 6 industrial-grade docs landed

- `docs/security.md` — threat model + sandbox boundaries.
- `docs/migration-v1-to-v2.md` — scaffold with TBD placeholders
  per breaking-change category, fills land at ship.
- `docs/aot.md` — AOT single-binary deploy guide (when / how /
  cross-compile / size breakdown / limitations / inspection).
- `docs/deploy.md` — production deployment patterns
  (crate selection / packaging shapes / runtime knobs / observability
  / graceful shutdown / cross-thread).
- `SECURITY.md` — formal CVE disclosure policy (email
  `admin@golia.jp`, 90-day default window).
- `CONTRIBUTING.md` — formal no-external-contrib policy
  (single-maintainer; PRs closed without review; fork freely under
  MIT/Apache-2.0).
- `docs/embedding.md` `vm.open_io()` / `vm.open_os()` stale API
  references corrected to `vm.open_os_io()`.
- `docs/architecture.md` crate layout refreshed from v1.1's 2-crate
  table to current 5 publishable + 2 dev-only; steel-cement-stone
  classification updated with actual file paths and v2.0 sprint
  discipline anchors.

#### AOT polish 6 verdict (AO-PF)

- Runtime counter added to chain reloc fire path
  (`crates/luna-runtime-helpers/src/lib.rs`).
- JIT in-process fib(28): **162,851 fires / 434,279 dispatches** —
  Stage 7 polish 6 alive on the JIT side.
- AOT-binary workload battery (fib(20), sum(1000), inlined helper,
  counted loop, GetField loop): **0 fires across all 5** — Stage 7
  polish 6 effectively dead in AOT, **but not the polish itself**:
  the AOT recorder filter (`dispatchable=false` for self-recursive
  traces) keeps input from ever reaching it. Verdict + handoff at
  the AO-PF verdict. **Not reverted** (JIT side
  active); recorder fix deferred to Track R landing.

---

## [1.3.0] — 2026-06-25

> **Released** — 2026-06-25 to crates.io. All five workspace crates
> shipped at `= 1.3.0`:
> [`luna-core`](https://crates.io/crates/luna-core/1.3.0) ·
> [`luna-jit-derive`](https://crates.io/crates/luna-jit-derive/1.3.0) ·
> [`luna-jit`](https://crates.io/crates/luna-jit/1.3.0) ·
> [`luna-runtime-helpers`](https://crates.io/crates/luna-runtime-helpers/1.3.0) ·
> [`luna-aot`](https://crates.io/crates/luna-aot/1.3.0). GitHub
> release: <https://github.com/goliajp/luna/releases/tag/v1.3.0>.

**Mega sprint** — 2026-06-24 user directive collapsed the planned
v1.2.0 + v1.3.0 + v1.4.0 + parts of v2.0 into a single ship under
the `nodefer` upgrade ("nothing is deferred to v1.4 or later").
Headline phases:

- **Phase A** (was v1.2): `LuaUserdata` trait sugar, REPL multi-line
  + history, lint debt cleared, Track B/L/P/R/S/G floor — already
  on develop (commits `bc088bd` / `65ca2cc` / `70c4bff`).
- **Phase B-N** (v1.3 expanded): PUC luac body 5.1-5.5, Send-safety
  full impl, perf attack round 2 (Path B math-fold extend), wasm32-
  wasip1 port, true `obj.x` field-style + `derive(LuaUserdata)`,
  REPL tab + syntax highlight, async natives in dispatch, userdata
  Trace-bearing host payloads, host_roots slot recycling, **luna-aot
  native-binary compile**, **MacroLua dialect support**.

See the v1.3 charter for the full track list, time
window estimate, and Phase ordering. `nodefer` is the operating
contract: every line item ships in v1.3 or is documented as
permanently out-of-scope (currently only the `luna` crates.io name
reclaim falls there — sticking with `luna-jit`).

The Phase A content below was previously written under the
`[1.2.0]` heading; it ships now as part of v1.3.0 without a
separately-published v1.2.0 on crates.io.

### Phase A headline

Polish + ergonomics on the v1.1 ship. **`LuaUserdata`
trait sugar** for Lua-callable host types, REPL gets multi-line input
plus history, lint debt cleared, perf attack discovers the real
bottleneck (interp, not trace) and updates the methodology accordingly.

### Track B — `LuaUserdata` trait (new embedder surface)

- **`luna_core::vm::userdata_trait`** module exposes the
  [`LuaUserdata`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/trait.LuaUserdata.html)
  trait + [`UserdataMethods<T>`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/trait.UserdataMethods.html)
  builder + [`MetaMethod`](https://docs.rs/luna-core/1.3/luna_core/vm/userdata_trait/enum.MetaMethod.html)
  enum. Embedders register methods (`add_method` / `add_method_mut`),
  static fns (`add_function`), metamethods (`add_meta_method`), and
  call-syntax field getters (`add_field_method_get`) via a typed
  builder.
- **Per-Vm metatable cache** keyed by `TypeId::of::<T>()`. First
  `create_userdata::<T>` triggers `T::add_methods` once; subsequent
  instances reuse the cached `Gc<Table>`. Pinned via `pin_host` so
  GC keeps the metatable live.
- **`Vm::create_userdata` / `Vm::set_userdata` bound tightened** from
  `T: Any + 'static` to `T: LuaUserdata`. **BREAKING**: existing
  B8 users upgrade with `impl LuaUserdata for MyType {}` (one line).
- **Auto-install metatable + `__gc` finalizer wire** at userdata
  allocation time (`check_finalizer_userdata` called from
  `create_userdata`).
- **`FromLuaArgs::from_lua_args_skip_self`** added — the
  method-call shape where slot 0 is the receiver.
- **`FromLuaArgs for Vec<Value>`** — variadic decoder for
  dispatcher-style natives (e.g. `redis:call(cmd, ...)`).
- Three new runnable examples:
  `examples/userdata_demo.rs` (Counter), `userdata_vec3.rs`
  (arithmetic metamethods), `userdata_redis_stub.rs` (dogfood §4.1
  shape — state IS the payload, no `thread_local!`).
- `docs/embedding.md` §7 rewritten with subsections covering trait
  shape, static constructors, variadic dispatch, the v1.2 field-style
  limitation (call-syntax only — true `obj.x` deferred to v1.3, see
  Deferred section), GC ordering, and trait contract reminders.

### Track R — REPL

- **Multi-line continuation**: incomplete statements (detected via
  `SyntaxError::msg.contains(" near <eof>")`) emit `>>` and accept
  another line. `local x = function()` + `return 1` + `end` now
  works at the REPL instead of erroring on line 1.
- **`~/.luna_history` persistence**: 1000-entry capped history,
  loaded on startup, saved on exit. No new dependency
  (`std::env::var_os("HOME")` only).

### Track L — Lint debt cleared

- `cargo fmt --all` clean (cleared 606-site formatter drift from v1.0/v1.1).
- `cargo clippy --workspace --all-targets -- -D warnings` clean
  (12 historic errors fixed: 8 `not_unsafe_ptr_arg_deref` justified
  with rationale, 2 `approx_constant` → `std::f64::consts::PI`,
  1 ZST `uninit_assumed_init` constant-folded guard, 2 dialect-test
  fixture allows).
- `cargo fix` unused-imports sweep across 60+ files plus 5 hand-fixed
  clippy issues (unnecessary unsafe, match→unwrap_or_default, etc.).
- Workspace `[lints.clippy]` policy in `Cargo.toml` declares the
  strict baseline and the few documented exemptions
  (`missing_safety_doc` — `docs/unsafe-accounting.md` is SoT;
  `incompatible_msrv`, `too_many_arguments`, etc.).

### Track P — Perf attack (real bottleneck identified)

- **D2 criterion infra** + Linux CI runner workflow_dispatch
  perf-gate (manual trigger; `redis_lua_shape` baseline).
- **D3 TA1 Path B lowerer**: `GetTabUp` admitted into the trace
  recorder as a standalone helper (was: unconditional bail at
  `trace.rs:3030`). Traces compile end-to-end on the token-bucket
  shape; bail rate 0.
- **D4 A1 GetField fast path**: `Table::get_str` + Op::GetField
  interp arm skip `op_index` when the receiver is a known `Value::Str`
  with no metatable (commit `a2c98ae`).
- **`Vm::current_op`** API (ergo.rs) + `diag_opcode_breakdown.rs`
  example — runtime opcode counter for `[perf-decomposition-vs-polish.md]`
  §2 Phase A "actual workload validates the decomp" hard gate.
- **Methodology lesson** (`docs/performance.md` + global
  methodology doc updated): the v1.0 charter hypothesis "1.5×
  gap vs PUC 5.1 on token_bucket" was 4× understated. PUC 5.5 is
  ~4.1× faster than luna interp on the shape; LuaJIT 2.1 is ~196×.
  **True attack surface = interp,  not trace.** Trace JIT does not
  engage on the Redis-Lua-shape workload (`infer_getx_exit` returns
  None on the `Call(Native math.min)` mid-body; length-gate kicks in
  on short bodies). D4 A3/A4/A5 + Path B math-fold extend recorded
  as Deferred-to-v1.3 (NOT silent — see Deferred).

### Track S — `feature = "send"` framework reserved

- `[features] send = []` declared in `crates/luna-core/Cargo.toml`.
  Building with `--features send` triggers `compile_error!` pointing
  to `v1.3-rfc-send-arc.md`. Embedders can feature-detect (`cargo
  add luna-core --features send` fails loudly) without waiting for
  the v1.3 implementation.
- Phase 0 audit (`v1.2-audit-send-cost.md`): ARM M-series ~10%
  overhead (within RFC 15% ceiling); x86_64 Linux ~20% (refines the
  RFC ceiling, needs `SendVm` newtype fork in v1.3).

### CI / release infra (Track G)

- **Lint gate**: `cargo fmt --check` + `cargo clippy --workspace
  --all-targets -- -D warnings` on every push.
- **0-dep gate**: `cargo tree -p luna-core --prefix none` must show
  exactly one line (luna-core itself). Catches accidental
  dependency creep at PR time.
- **Unsafe-drift gate** (new in v1.2): first-party unsafe site count
  must stay under a recorded ceiling (490, baseline 461 from v1.1
  + ~15 from Track B). Bump the ceiling explicitly when justified;
  never widen to silence drift.
- `branches: [main]` → `[master, develop]` to track git-flow setup.
- `docs/release-checklist.md` (new) — version-agnostic checklist
  template; sprint-specific audits stay in the maintainer's local area.
- A discussion note archives the
  v1.1.0 ship-time rename story (`luna` → `luna-jit`).

### Phase B-N — v1.3 expansion in flight

Per the 2026-06-24 `nodefer` directive every item below is **in
scope** for v1.3 (no longer deferred). Tracked in
the v1.3 charter and plan state:

- **Path B math-fold extend** (`min` / `max` 2-arg) — *(landed Phase P2A)*
  `trace.rs::try_match_trace_math_fold` extended with `FoldKind::Min2 /
  Max2`. Split-window recognizer (only `GetTabUp + GetField + Call`
  flagged in `folded_ops` — arg-prep ops execute normally). Cranelift
  `smin/smax` for Int/Int, `fmin/fmax` for Float-or-mixed.
  `trace_dispatched_count` flipped 0 → 200/200 on `diag_token_bucket`.
  **TA3 default flip done** — `jit_state.rs::with_null_backend` ships
  `trace_enabled = true` (was `false`) after Linux taskset perf-gate
  confirmed `redis_lua_shape ≥ 1.0×` v1.2 baseline. Embedders that want
  the v1.2 interp-only default call `vm.set_trace_jit_enabled(false)`.
- **D4 A3 / A4 / A5** (newindex double-walk collapse / Move
  elimination / dispatcher reshape) — perf polish on top of A1.
- **`add_field_method_set` + true `obj.x` field-style access** —
  *(Phase UD1+UD2 landed)* `add_field_method_set(name, fn)` registers
  setters for `obj.name = value`; the `__index` slot becomes a native
  trampoline when any getter is registered, so `obj.width` (no parens)
  resolves to the field value directly. **Breaking change** from the
  v1.2 sugar: `obj:name()` call-syntax for `add_field_method_get` no
  longer works (the trampoline calls the getter and returns
  `Int(...)`, so `Int(...)(obj)` errors). Embedders who need both
  shapes should register an explicit `add_method("name", ...)`
  alongside the field-getter. Unknown writes go to a runtime error
  rather than silently dropping.
- **`#[derive(LuaUserdata)]` proc-macro** — *(Phase UD3 landed)* new
  `luna-jit-derive` crate ships the derive + `#[lua_userdata_methods]`
  attr macro. Helper attributes: `#[lua_method("name")]`,
  `#[lua_method_mut]`, `#[lua_function]`, `#[lua_meta_method(Add)]`,
  `#[lua_meta_method_mut]`, `#[lua_field_get]`, `#[lua_field_set]`,
  `#[lua_skip]`, plus struct-level `#[lua_type_name = "X"]`. Hand
  impl stays as the escape hatch (generic types, conditional method
  sets). luna-core 0-dep contract preserved — derive lives in
  `luna-jit-derive` only; luna-jit's build-time supply chain grows by
  `syn + quote + proc-macro2` (the standard derive trio). `cargo
  tree -p luna-core --prefix none --no-default-features` still
  reports 1 row. Embedders writing `use luna_jit::LuaUserdata;` get
  both the trait (via the `pub use luna_core::*;` re-export) and the
  derive (`pub use luna_jit_derive::LuaUserdata;`).
- **`feature = "send"` real implementation** *(Phase SS-B landed)*
  — new opt-in cargo feature on luna-core (`send = []`) and
  luna-jit (`send = ["luna-core/send"]`) surfaces a second public
  type `luna_core::vm::SendVm` for cross-thread embedding. Shape:
  `SendVm { inner: Arc<UnsafeCell<Vm>>, lock: Arc<RwLock<()>> }`
  with `unsafe impl Send for SendVm` (justified by a runtime
  single-mutator invariant the lock re-establishes). Default-feature
  builds are bit-identical with the pre-SS-B baseline — bare `Vm`
  stays `!Send + !Sync` and pays no overhead. luna-core 0-dep
  contract preserved (`Arc`, `UnsafeCell`, `RwLock` are all
  stdlib).
  - **API surface mirror**: `eval`, `call_value`, `set_global`,
    `set_userdata`, `intern_str`, `open_base / open_math /
    open_string / open_table / open_coroutine`, the Phase SR
    `pin_host / read_host / unpin` host-roots methods, plus
    `Clone` (cheap — two `Arc::clone`), `Debug`, and one new
    method `get_global(name) -> Value` that isn't present on bare
    `Vm` (introduced because the bare `globals()` + raw `Gc<Table>`
    deref is awkward across the lock boundary).
  - **Interp-only constraint**: `SendVm::new` calls
    `Vm::new_minimal` which leaves `JitState` at `NullJitBackend`.
    The trace JIT does not run on a SendVm in v1.3. JIT-aware
    SendVm is a documented post-v1.3 polish item (the
    `Proto::traces: RefCell<Vec<Rc<CompiledTrace>>>` field
    intersects with `Send` and would need an `Rc → Arc` migration;
    audit projects ~6 % additional JIT-engaged cost). Not a
    defer — the v1.3 charter explicitly scopes interp-only as the
    SS-B deliverable.
  - **Cost** (macOS M-series, SS-B bench): SendVm pays ~+1.86 %
    token-bucket regression vs interp-only baseline `Vm` (175.46
    µs vs 172.26 µs). Better than the audit's projected ~3 % ARM.
    Linux x86_64 numbers land via the `perf-gate` CI matrix
    (audit projects ~6 %).
  - **8 smoke tests** in `crates/luna-core/tests/send_vm.rs`
    (gated `#[cfg(feature = "send")]`): compile-time `Send`
    assertion, basic eval, `thread::spawn` move, 100-thread
    concurrent contention (verifies serialized counter = 4950),
    userdata round-trip, HostRootTicket round-trip across the
    lock, pin-across-clones, and interp-only loop sum.
  - **Bench update** (`crates/luna-jit/benches/bench_send_overhead.rs`):
    feature-gated `send_vm_eval` and `send_vm_token_bucket` pairs
    added alongside the SS-A `wrapped_vm_*` NoOpWrapper baseline;
    apples-to-apples interp-bare counterparts (`bare_vm_interp_*`)
    added for the SendVm comparison.
  - **Documentation**: `docs/threading.md` gains a `SendVm`
    section covering when to use vs not, the shape + soundness
    story, the interp-only constraint, and a tokio multi_thread
    embed example (without depending on tokio in luna-core).
  - **Design RFC** documents
    the as-shipped wrapper choice + the decision to defer the
    audit's per-field `SendGc<T>` fork to v1.4+.
  - **Unsafe drift**: +5 first-party `unsafe` sites (480 → 485,
    ceiling 490 — 5 slots free). New sites: `unsafe impl Send for
    SendVm` (one), `&mut *UnsafeCell::get()` inside
    `with_vm_mut` (one), `(*globals.as_ptr()).get(key)` in
    `get_global` (one), two doc-comment occurrences caught by the
    grep regex.
  - **BREAKING vs v1.2 stub**: the v1.2 `[features] send = []`
    that raised a `compile_error!` when selected now compiles
    cleanly and surfaces `SendVm`. Embedders who were guarding
    against the compile_error with `cfg(not(feature = "send"))`
    no longer need that guard.
- **REPL C3 tab completion + syntax highlight** — `[features]
  repl-line-editor` (rustyline) non-default cargo feature.
- **PUC luac body 5.1-5.5** — full binary compat across all
  shipping Lua dialects; opt-in `Vm::set_puc_bytecode_loading(true)`
  + per-dialect translator under `crates/luna-core/src/vm/dump/`.
- **wasm32-wasip1 support** — `io.popen` / `os.execute` cfg-gated
  + wasi stubs return PUC error tuple.
- **`official_run` flakiness fix** — compiler short-circuit AND
  `debug_assert_eq!(reg, base)` + sweep misaligned-pointer cascade
  root cause + fix.
- **Async natives in dispatcher** (B11 hook firing) *(Phase AS
  landed)* — close the v1.1 B10 Stage 2 deferred path so async-marked
  natives compose with Rust-side `[B11]` debug hooks. Audit
  showed the gap was
  narrower than the v1.1 charter assumed: the dispatcher hot loop's
  `Count` / `Line` / Lua-`Call` / Lua-`Return` sites are opcode-driven
  and already fire correctly under `async_mode = true`; only the
  async-native call boundary itself was missing. Phase AS adds:
  - `Call` event on the async-native branch in
    `crates/luna-core/src/vm/exec.rs`, fired after the
    `native_nresults` / `gc_top` pin and before the future is built —
    same placement-relative-to-pin as the sync native path's
    `hook_call(true, nargs)` site (audit §A.1 / Q6).
  - `Return` event in `Vm::commit_async_native_result`
    (`crates/luna-core/src/vm/async_drive.rs`), fired after
    `finish_results` lands the resolved nret into the call window and
    before the post-call GC checkpoint. Mirrors the sync native's
    `hook_return(true, nargs + 1, nret)` placement. The method is
    now fallible — `EvalFuture::poll`'s `Poll::Ready(Ok(nret))` arm
    propagates the hook error through the same JIT-restore + cleanup
    path the `Poll::Ready(Err)` arm already runs.
  - **Count + Line carryover** — no code change; the dispatcher's
    persistent `hook.count_left` and `hook_lastline` `Vm` fields
    already carry across `Poll::Pending` returns to the executor, so
    a 1000-instruction count budget walks down naturally across
    arbitrarily many slice boundaries and a line event won't
    double-fire on resume mid-line. New tests pin both as regression
    guards.
  - **6 smoke tests** in
    `crates/luna-core/tests/async_hook_composition.rs`: `Call`/`Return`
    around an immediate-Ready async native, `Call`/`Return` bracketing
    a yield-once async native (proves the Return fires after `.await`
    resolves), count-hook carryover across an aggressive 50-op slice,
    line-hook dedupe across a 3-op slice, compile-time
    `assert_send::<RustDebugHook>()` + `assert_sync::<RustDebugHook>()`
    pinning the function-pointer Send-safety property, and a
    composition smoke confirming the hook body observes the
    async-native Call event end-to-end. No tokio dep — same
    hand-rolled `block_on` + `YieldOnce` harness as the existing
    `tests/async_native.rs` (luna-core 0-third-party-dep contract
    preserved).
  - **`Send` composition with SS-B** — `RustDebugHook = fn(&mut Vm,
    RustHookEvent)` is a bare function pointer and unconditionally
    `Send + Sync`, so the v1.3 Phase SS-B `SendVm` newtype composes
    cleanly with async hooks without any new trait bound. The
    compile-time `assert_send` test is the regression guard for any
    future evolution of the hook signature toward closure state.
  - **Re-entrancy contract**: hook bodies under async mode may call
    sync `vm.eval(...)` but must NOT invoke async natives — the
    inner sync `eval` lacks an executor to drive a nested
    `EvalFuture`, and the existing rejection
    (`"async native called in sync context"`) catches the attempt
    cleanly. Documented in `docs/threading.md` §"Async natives +
    debug hooks".
  - **Q5 followup** (audit §"Open questions"): `EvalFuture::Drop`
    already clears `pending_async_native_fut` /
    `pending_async_native_ctx` (`async_drive.rs:553-554` in the
    pre-AS code), so the stale-ctx hardening the audit flagged is
    already in place — no additional cleanup required in Phase AS.
  - **Unsafe drift**: 0 new sites. Hook visibility bump from `fn`
    to `pub(crate) fn` on `Vm::hook_call` and `Vm::hook_return` is
    safe-Rust-only.
- **Userdata `Trace`-bearing host payloads** — `T` may hold
  `Gc<...>` fields; collector recurses into the payload (userdata
  GC ripple).
- **`host_roots` slot recycling** *(Phase SR landed)* — the v1.1
  append-only `Vec<Value>` is replaced by a free-list-backed slot
  pool keyed by `HostRootTicket { idx: u32, generation: u32 }`
  (8 bytes, `Copy`). `pin_host` returns the ticket; `unpin` clears
  the slot to `Nil`, bumps generation, and pushes the index onto
  the free list for reuse; `read_host` / `write_host` validate the
  ticket's generation and return `None` / `Err(HostRootStale)` on
  stale lookup (ABA-safe). Generation overflow at `u32::MAX` retires
  the slot permanently (bounded leak: ~4 days at 10⁹ unpins/day per
  slot). Long-running embedders (request-per-script loops, edge
  workers) now hold at a bounded pool size instead of growing the
  vector monotonically.
  - **BREAKING — embedder Vm API**: `Vm::pin_host(v: Value) -> usize`
    is now `Vm::pin_host(v: Value) -> HostRootTicket`;
    `Vm::host_root_at(idx) -> Value` and `Vm::host_root_set(idx, v)`
    are **removed** in favor of `Vm::read_host(t) -> Option<Value>`
    and `Vm::write_host(t, v) -> Result<(), HostRootStale>`. New
    methods: `Vm::unpin(t) -> Result<(), HostRootStale>`. Existing
    `Vm::unpin_all()` and `Vm::host_root_count() -> usize` signatures
    unchanged; `unpin_all` semantics extended to bump every slot's
    generation (all outstanding tickets become stale uniformly).
    Migration: replace stored `usize` index with `HostRootTicket`;
    `vm.host_root_at(idx)` → `vm.read_host(ticket).expect("...")`;
    `vm.host_root_set(idx, v)` → `vm.write_host(ticket, v).unwrap()`.
  - **BREAKING — `luna-jit` facade structs**: `LuaFunction` /
    `LuaTable` / `LuaRoot` now carry `ticket: HostRootTicket`
    (was `idx: usize`). `Copy + Clone` preserved; public method
    surface (`call` / `call_multi` / `get` / `set` / etc.) is
    invariant. New `Lua::unpin(handle)` releases a single handle
    via the new `PinnedHandle` trait (impl'd by all three handle
    types). Reads after `Lua::unpin` / `Lua::unpin_all` panic with
    `"<HandleType> used after unpin / unpin_all"` — matches the v1.1
    "handles created before `unpin_all` become invalid" docstring.
  - New module: `luna_core::vm::host_roots` (own the pool impls);
    types re-exported as `luna_core::vm::{HostRootTicket, HostRootStale}`.
    Tests: `crates/luna-core/tests/host_roots_slot_recycling.rs`
    (10 tests covering basic recycle, ABA detection, `unpin_all`
    invalidation, 100k pin/unpin smoke, free-list LIFO, GC tracer
    correctness across recycle).
- **`luna-aot` native-binary compile** *(Phase AOT scaffold landed;
  Cranelift codegen follow-up within v1.3)* — new sibling crate
  `crates/luna-aot/` (workspace member alongside `luna-core` +
  `luna-jit` + `luna-jit-derive`). Ahead-of-time compiler that
  emits a self-contained binary embedding the Lua bytecode with no
  runtime parse step.
  - **Scaffold pipeline end-to-end** today: Lua source →
    `luna_core::frontend::parser::parse` → AST →
    `luna_core::compiler::compile_chunk` → `Gc<Proto>` →
    `luna_core::vm::dump::dump` → luna body dump bytes →
    `object::write::Object` with a `.luna.bytecode` ReadOnlyData
    section bracketed by global symbols
    `__luna_bytecode_start` / `__luna_bytecode_end` (Mach-O
    `_`-prefixed) → system `cc` link with a minimal C entry +
    bytecode `.o` → host-triple native binary that prints the
    embedded section length to `stderr` (proves the section is
    reachable end-to-end).
  - **CLI**: `luna-aot compile <input.lua> [--out <path>]
    [--target <triple>] [--dialect 5.1|5.2|5.3|5.4|5.5|macrolua]`.
    `clap` derive surface; scaffold rejects non-host `--target`
    until Stage 6 cross-compile lands.
  - **Library surface**: `luna_aot::embed::embed_bytecode(source,
    out, target_triple, version)` for programmatic embedders;
    `luna_aot::runtime_stub::aot_main()` (interp-driven Vm
    entry — compiles cleanly, awaiting wire-up to the link step in
    the follow-up session via cargo-bootstrap or staticlib
    distribution per audit § Stage 6 Option A/B); constants
    `BYTECODE_START_SYMBOL` / `BYTECODE_END_SYMBOL` /
    `BYTECODE_SECTION_NAME`.
  - **Supply-chain delta**: `luna-aot` pulls `object 0.36`
    (`default-features = false`, `elf` + `macho` + `pe` + `write_std`)
    + `clap 4` (derive) + dev-only `tempfile 3`. **luna-core
    0-third-party-dep contract is unaffected** — `cargo tree -p
    luna-core --prefix none --no-default-features | grep -cE " v[0-9]"`
    still reports 1. Workspace-wide transitive growth = ~50 crates
    (clap + object + their derive transitives). cargo-deny config
    may want a `[bans] multiple-versions = "warn"` pass; flagged
    for the follow-up phase that adds the per-crate deny job.
  - **Test**: `crates/luna-aot/tests/scaffold_smoke.rs` exercises
    the end-to-end path (parse + compile + dump + `.o` write + `cc`
    link → on-disk non-empty native binary). Does not execute the
    binary — the scaffold's C entry's stderr-only output isn't a
    correctness signal for this session; the runtime-stub follow-up
    adds the stdout-comparison test.
  - **Phase AOT Stage 3 — backend-agnostic lowerer** *(landed in
    this commit)*. Both `lower_int_chunk_into<M: Module>` and
    `lower_trace_into<M: Module>` in `luna-jit::jit_backend` are now
    generic over `cranelift_module::Module`, so the same codegen
    body drives the runtime `JITModule` (live RWX mmap) and the AOT
    `ObjectModule` (`.o` file emission). The JIT-specific module
    construction (`JITBuilder::with_isa` + `builder.symbol("luna_jit_*",
    …)`) is factored into thin helpers `build_jit_module_with_helpers`
    (int-chunk) + `build_trace_jit_module` (trace), keeping
    `JITModule::finalize_definitions` /
    `get_finalized_function` / `TRACE_JIT_HANDLES` insertion isolated
    in the JIT wrappers. The two trace-lowering free fns
    `emit_table_set` / `emit_materialize_live_sunk` are now also
    generic over the module trait. Trace returns place a
    `placeholder_trace_fn` in `CompiledTrace.entry`; the JIT wrapper
    patches the real fn pointer after finalize, while the AOT
    pipeline resolves the symbol at static-link time and never
    invokes `entry` directly. A new smoke test
    `crates/luna-aot/tests/stage3_lower_into_object.rs` drives the
    int-chunk lowerer with `cranelift_object::ObjectModule` and
    asserts the produced bytes carry the host's object-file magic
    number — load-bearing witness that the generic boundary is
    actually consumed by a second backend, not just claimed.
    Helper-symbol registration is JIT-only for now; the AOT pipeline
    will resolve these via static link against a small
    `luna-runtime-helpers` rlib in a follow-up (audit § Stage 3
    Action item 3). 274 / 274 workspace lib tests + 360+ luna-jit
    integration tests stay green; the pre-existing
    `trace_jit_s1` failures (2 / 4, baseline-drift from the TA3
    `trace_enabled = true` ship default) and the known
    `official_run` SIGABRT (IO Safety fd-double-close, see
    the IO-safety known bug) are unchanged
    by this refactor.
  - **Phase AOT Stage 4 — linker + interp-runtime staticlib**
    *(landed in this commit)*. A new sibling crate
    `crates/luna-runtime-helpers/` ships as a dual
    `crate-type = ["staticlib", "rlib"]` library that depends only
    on `luna-core` (luna-core's 0-third-party-dep contract is
    unaffected — `cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"`
    still reports 1). It exposes one C-ABI symbol
    `#[unsafe(no_mangle)] pub unsafe extern "C" fn luna_aot_run(bytecode: *const u8, len: usize) -> i32`
    that constructs a `Vm::new(LuaVersion::Lua55)`, enables
    bytecode loading, calls `Vm::load(slice, b"=embedded")`, runs
    `call_value` on the root closure, and returns the process exit
    code (0 success / 1 load-or-runtime-error / panics caught and
    reported). The new `luna_aot::embed::compile_and_link`
    function in `crates/luna-aot/src/embed.rs` drives the full
    deploy pipeline: parse → compile → dump → bytecode `.o` → C
    `main.c` (extern-decls the bracket symbols + `luna_aot_run`,
    emits `cc -c main.c -o main.o`) → `cargo build -p
    luna-runtime-helpers --release` (or `LUNA_AOT_RUNTIME_HELPERS_STATICLIB`
    env override for distribution scenarios; in-process `Mutex`
    serialises concurrent in-test callers against cargo's atomic-
    rename window) → `cc bytecode.o main.o libluna_runtime_helpers.a
    [platform libs] -o <out>` (mac: `-framework CoreFoundation
    -framework Security -liconv`; linux:
    `-lpthread -ldl -lm -lrt -lgcc_s -lutil`; windows: explicit
    `AotError::Link` — Windows folds into the cross-compile
    follow-up). The CLI's `compile` subcommand routes through
    `compile_and_link` by default; the prior scaffold path
    (C-entry-only, prints section length to stderr) remains
    reachable via `--scaffold-only` for users who want to
    benchmark the link step in isolation. New test
    `crates/luna-aot/tests/stage4_link_and_run.rs` covers three
    end-to-end scenarios: `print('hello from aot')` lands on
    stdout with exit 0; arithmetic + multi-print
    (`print(5); print('done', 10)`) produces the expected
    tab-separated PUC-shape output; `error('boom')` propagates as
    exit 1 with the message on stderr. Tests skip cleanly on
    Windows / missing-`cc` hosts (Stage 4 ships Unix-only).
    Stage 5 Cranelift trace-mcode emission and Stage 6
    cross-compile remain follow-ups.
  - **Phase AOT Stage 5 — cross-compile via `--target`** *(landed in
    this commit)*. New public `luna_aot::embed::TargetSpec` resolves
    a triple string into the per-target bundle the pipeline needs:
    `object` format (ELF / Mach-O / PE), arch, endianness, OS family
    (`TargetOs::{MacOs, Linux, Windows}`), libc flavour
    (`TargetLibc::{Default, Musl, MinGw}`), and the right `cc`
    driver. Resolution prefers a named cross-cc on PATH
    (`aarch64-linux-gnu-gcc`, `x86_64-w64-mingw32-gcc`,
    `x86_64-linux-musl-gcc`, ...) then falls back to `cc -target
    <triple>` (works on macOS hosts where Apple's clang accepts
    `-target` natively). `build_runtime_helpers_staticlib` now takes
    `Option<&str>` and shells out to
    `cargo build --target=<triple> -p luna-runtime-helpers --release`
    when a non-host triple is requested; the resulting staticlib
    lands at `target/<triple>/release/libluna_runtime_helpers.a`
    (or `luna_runtime_helpers.lib` on Windows). The final link uses
    a per-OS lib set: macOS keeps `-framework CoreFoundation
    -framework Security -liconv`; glibc Linux keeps the Stage 4
    `-lpthread -ldl -lm -lrt -lgcc_s -lutil` set; musl Linux drops
    `-lrt -lgcc_s -lutil` (those symbols are inside musl libc);
    Windows-MinGW adds `-luserenv -lkernel32 -lws2_32 -lbcrypt
    -ladvapi32 -lntdll` (the rust stdlib's win32 shim deps as
    reported by `rustc --print native-static-libs`). Tier 1
    (verified end-to-end on macOS aarch64 host): host triple +
    `x86_64-apple-darwin` cross. Tier 2 (codegen + link wired,
    self-skip when host cross-cc is missing):
    `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`,
    `x86_64-unknown-linux-musl`, `x86_64-pc-windows-gnu`. New test
    `crates/luna-aot/tests/stage5_cross_compile.rs` covers seven
    cases: pure-unit triple-parser smoke (`target_spec_parses_tier1_triples`),
    unsupported-arch rejection (`target_spec_rejects_unsupported_arch`),
    plus one `cross_compile_*` test per tier-2 triple. Each
    per-triple test reads the produced binary's leading bytes and
    asserts the object-file magic matches the requested format
    (ELF `\x7fELF`, Mach-O `0xfeedfacf` / `0xcffaedfe`, PE `MZ`).
    All tests self-skip with informative `eprintln!` lines when
    rust-std or cross-cc isn't installed; the test list is green
    on a generic dev box without any cross-toolchains.
  - **Phase AOT Stage 5 — Windows linker** *(landed in this commit)*.
    The Stage 4 hard error `"Windows linker support not implemented"`
    is replaced with two clear paths: MinGW
    (`x86_64-pc-windows-gnu` → `x86_64-w64-mingw32-gcc`) is wired
    through the regular target-aware `cc` driver pick + the
    Windows-MinGW lib set; MSVC (`x86_64-pc-windows-msvc`) returns
    `AotError::Link` with a concrete workaround message
    ("target `x86_64-pc-windows-gnu` instead, or run
    `--scaffold-only` and invoke link.exe by hand"). The MinGW path
    is exercised by `stage5_cross_compile::cross_compile_x86_64_pc_windows_gnu`,
    which self-skips when `x86_64-w64-mingw32-gcc` isn't on PATH.
  - **Phase AOT Stage 6 — Alpine no-Lua deploy smoke** *(landed in
    this commit)*. Charter AOT6 closure. New test
    `crates/luna-aot/tests/stage6_alpine_smoke.rs` builds
    `hello.lua` for `x86_64-unknown-linux-musl`, runs the
    resulting binary inside an `alpine:3.20` container with **no
    Lua installed** (no `apk add lua*`), and asserts stdout matches
    the expected `print(...)` output. A best-effort secondary
    `verify_only_musl_libc` step uses busybox `strings | grep` to
    confirm the binary doesn't reference `liblua` or `libluna`.
    Self-skips cleanly when any prerequisite is missing:
    docker/podman daemon (tries both), rust-std for the musl
    triple, musl cross-cc, network access to `docker.io`. The skip
    paths print one-line `eprintln!` install hints (`brew install
    FiloSottile/musl-cross/musl-cross` for macOS, `apt install
    musl-tools` for Debian).
  - **Final phase remaining**: trace JIT mcode emission via
    `cranelift-object` (walk every reachable `Proto`'s hot loops,
    drive each through the Stage 3 generic lowerer, emit symbols +
    dispatch table into the AOT binary). The interp staticlib
    runtime already carries the fallback so trace.o is purely
    additive; this is post-v1.3.
- **MacroLua dialect support** — Lua syntax extension as an
  optional dialect alongside 5.1-5.5; routed through the existing
  per-dialect lexer/parser machinery so it doesn't disturb the
  PUC compatibility matrix.

### Permanently out-of-scope (decision 2026-06-24)

- **Reclaim `luna` crate name on crates.io** — abandoned; sticking
  with `luna-jit` for the JIT-equipped crate and `luna-core` for
  the 0-dep interpreter. See
  a discussion note kept with the project's private records.

### Internal — sprint methodology

- The perf baselines from 2026-06-24 record the decomp work
  that surfaced "interp not trace" as the true attack surface.
- The perf-attack methodology gained an anti-pattern catalog drawn
  from the v1.0 fib_28 misdirection.
- Charter, plan-state and audit docs live in the maintainer's local area
  (gitignored); `docs/` stays user-facing.

---

## [1.1.0] — 2026-06-23

### Ship-time crate rename

The JIT-equipped crate is published as **`luna-jit`** instead of
`luna` because the `luna` name on crates.io is taken by an
unrelated utilities library. The directory layout, library
exports, and CLI binary name (`luna`) are unchanged; only the
crate name visible on crates.io is `luna-jit`. Embedders use:

```toml
[dependencies]
luna-jit = "1.1"   # or:   luna-core = "1.1"   for the 0-dep core
```

```rust
use luna_jit::Lua;   // (was `use luna::Lua;`)
```

The CLI binary still installs as `luna` (`cargo install luna-jit`
puts a binary named `luna` on PATH). `luna-core` keeps its name
(0-dep interpreter is the pure thing).

### Track A — Crate / Dep / Safety

- **Workspace split** (A1): `luna-core` (0 third-party deps; lexer /
  parser / compiler / interpreter / runtime / stdlib / GC / pattern /
  JIT trait surface) and `luna` (Cranelift JIT + capi + CLI binary).
  `cargo add luna-core` pulls only the interpreter; `cargo add luna`
  pulls the full JIT'd stack. CI gate: `cargo tree -p luna-core`
  must show exactly one crate.
- **JIT trait boundary** (A1 Session A): `IntChunkCompiler` /
  `TraceCompiler` traits in `luna_core::jit::abi` decouple the
  dispatcher from Cranelift. `NullJitBackend` (in `luna-core`) and
  `CraneliftBackend` (in `luna`) implement the traits.
- **`Vm::new_minimal_with_jit`** in the `luna` crate — one-line
  constructor for embedders wanting the v1.0 JIT-on-by-default
  behavior through `cargo add luna`.
- **`Vm` rustdoc + `!Send` compile_fail doctest** (A7) — `Vm: !Send + !Sync`
  is now CI-enforced. `docs/threading.md` covers canonical
  embedding patterns.
- **`JitState` sidecar** (A2): JIT-specific Vm fields factored into
  a dedicated struct, freeing the Vm hot path from JIT churn.
- **SAFETY: comment coverage** (A6): 100% across `unsafe { ... }`
  blocks. 342 new annotations added. See `docs/unsafe-accounting.md`.
- **Public API 0 unsafe** (A4): 4 `pub unsafe fn` items demoted to
  `#[doc(hidden)]`; `TableBuilder` / `IntoValue` / `native_typed`
  cover the safe embedder flows. The dogfood §4.1 friction is closed.
- **Panic-safe public boundaries** (A5): `Vm::set_global` returns
  `Result<(), LuaError>`; 68 call sites updated.
- **`cargo-deny`** (A3): CI workflow gates supply chain (advisories,
  licenses, source registry) plus a hard `luna-core` 0-dep check.

### Track B — Embedder API

- **`Vm::sandbox(version).build()`** (B1): Conservative-default
  sandbox builder; embedders whitelist stdlib modules + set
  instr/memory budgets in one chain.
- **`vm.eval` / `vm.eval_chunk`** (B2): Single-call source-to-value
  evaluation returning `Result<Vec<Value>, LuaError>`. SyntaxError
  surfaces as a heap-interned `LuaError`.
- **`TableBuilder` + `vm.table_of`** (B3): Build tables with chained
  `.with(k, v)` calls or a fixed-size slice. Embedders never write
  `unsafe { gc.as_mut() }` for table construction.
- **`IntoValue` trait** (B4): `vm.set_global("k", 42_i64)` infers;
  blanket impls cover `i64`, `f64`, `bool`, `&str`, `String`,
  `Vec<u8>`, `Gc<Table>`, `Gc<LuaClosure>`, `Gc<NativeClosure>`,
  `Value`, `()`, `Option<T>`.
- **`vm.native_typed` + `FromLuaArgs`/`IntoLuaReturn`/`FromLuaValue`**
  (B5): Typed Rust functions exposed as Lua callables. Arities 0-6,
  fn pointers and non-capturing closures, multi-value returns,
  `Result<T, LuaError>` for fallible natives.
- **Structured `LuaError`** (B6): Adds `LuaErrorKind` enum
  (Runtime / Syntax / InstrBudget / MemoryCap / Native /
  OutOfMemory / Type), `impl Display + Error` on `LuaError`,
  Vm-side `error_kind` / `error_source` / `take_error_traceback`
  accessors. `LuaError` stays `Copy`.
- **String interop** (B7): `vm.intern_str`, `Value::try_as_str`
  (UTF-8 validating), `Value::as_bytes` (binary-safe).
- **Host userdata** (B8): `vm.create_userdata::<T>(value)` /
  `set_userdata` / `userdata_borrow` / `Userdata::downcast` for
  arbitrary `T: 'static` host types. The closed-world userdata
  infrastructure now accepts host payloads.
- **Rust-side coroutine drive** (B9): `vm.create_coroutine` /
  `vm.resume_coroutine` parallel to `coroutine.create` / `:resume`.
- **Async embedder API** (B10): `vm.eval_async` returns a `!Send`
  Future driving the dispatcher with cooperative yields on
  instruction budget exhaustion. `vm.set_async_native` exposes
  async Rust functions to Lua scripts. `Lua::eval_async` /
  `Lua::set_async_native` mirror on the facade.
  `examples/async_host.rs` ships a runnable Tokio-substitute
  walkthrough. 0 new third-party deps (`std::future` + `std::task`
  suffice).
- **Rust-side debug hook** (B11): `vm.set_rust_debug_hook` accepts
  a `fn(&mut Vm, RustHookEvent)` plus mask flags
  (HOOK_MASK_CALL / RETURN / LINE / COUNT). Both Lua-side
  `debug.sethook` and Rust hooks can coexist.
- **`Lua` newtype facade** (B12): `mlua`-shape front door with
  owned `LuaFunction` / `LuaTable` / `LuaRoot` handles backed by
  an append-only `Vm::host_roots` pool. Use `Lua::new()` for the
  five-minute start; use `Vm` for the low-level handle.

### Track C — CLI / REPL

- **Interactive REPL** (C1): `luna` with no args drops into a
  single-line REPL. Each line is tried as an expression
  (`return <line>`), then as a statement on syntax error.
- **CLI flags** (C4): `--sandbox` builds via SandboxBuilder;
  `--budget=N` sets instr budget; `--no-jit` installs NullJitBackend;
  `--profile` prints trace-JIT counters on exit.
- **Pretty errors** (C5): Compile + runtime errors render with
  classified kind tag, source location, snippet, and traceback.
  ANSI color when stderr is a TTY and `NO_COLOR` is unset.

### Track D — Bench / Perf

- **Redis-Lua-shape micro-bench** (D1): New `redis_lua_shape` bench
  with four workload shapes from the dogfood report
  (`token_bucket_1k`, `sliding_window_500`, `method_dispatch_5k`,
  `string_ops_2k`).
- **`docs/performance.md` extension** (F4): D1 baseline added
  alongside the cross-dialect snapshot.

### Track E — Dialect / require / Compat

- **`docs/compatibility.md` extension** (E2): v1.1 luna-specific
  extension table + CLI options reference + REPL behavior.

### Track F — Docs

- `docs/architecture.md` (F5): crate layout + source classification
  + JIT pipeline + threading + sandbox.
- `docs/threading.md` (A7 artifact): `!Send` patterns + Tokio +
  async embedder API.
- `docs/embedding.md` (F1): 12-section embedder cookbook
  (install / hello / sandbox / globals / tables / native_typed /
  userdata / coroutines / debug hooks / errors / Lua facade /
  threading).
- `docs/binary-size.md` (G5): cargo-bloat snapshot
  (cranelift_codegen 45% / luna_core 25% / std 13%).
- `docs/unsafe-accounting.md` (G4): cargo-geiger companion;
  461 unsafe sites, 394 SAFETY-annotated, 6 pattern categories.
- README.md rewrite (F6): workspace + ergo + honest perf.

### Track G — CI / Release

- **MSRV declaration** (G1): `rust-version = "1.86"` in
  `[workspace.package]`; CI workflow `.github/workflows/msrv.yml`
  locks against it.
- **CI matrix** (G2): `.github/workflows/ci.yml` runs
  build/test/release/doc on Linux + macOS + Windows + wasm32
  (luna-core only). `cargo doc --workspace -D warnings` gate.
- **`cargo-deny`** (A3, listed above): supply-chain + 0-dep gate.

### Changed

- Source tree reorganization: `src/jit/trace.rs` (9483 LOC) split
  in place into `trace.rs` (Cranelift codegen body) and
  `trace_types.rs` (type definitions + thresholds + cranelift-free
  helpers). Type paths preserved via re-exports; downstream
  callers see no API change.
- `Vm::set_global` signature changed from
  `(&mut self, name: &str, v: Value)` to
  `<V: IntoValue>(&mut self, name: &str, v: V) -> Result<(), LuaError>`.
  Existing callers passing `Value::*` directly still compile (V
  infers to Value). New ergonomics: `vm.set_global("k", 42)`.

### Deferred to v1.2

- C2 (REPL multi-line continuation + history)
- C3 (REPL tab completion + syntax highlight, likely as
  `luna-repl` binary crate)
- D2 (criterion infra + n=1000 + CPU pin + 10 runs)
- D3 (token_bucket decomposition vs PUC 5.1)
- D4 (attack-agent perf workflow)
- E1 (require searcher table dispatch — behavior change requires
  PUC test re-verification)
- E3 (PUC `luac` body 5.1-5.5 compat — 20-30 day block, charter L)
- E4 (string.pack/utf8 edge case test gaps)
- Lint cleanup (`cargo fmt --all` 606 sites + 9 `clippy` errors,
  a known drift in historic fmt/clippy runs)
- `feature = "send"` `Arc<RwLock<T>>` sprint (see
  the v1.1 Send/Sync RFC)
- `LuaUserdata` trait sugar (B8 follow-on; closed-world ships
  v1.1, trait sugar lands later)

---

## [1.0.0] — 2026-06-23

First stable release. luna implements **Lua 5.1, 5.2, 5.3, 5.4, and
5.5** in pure Rust with zero non-build dependencies (cranelift is
the JIT codegen).

### Correctness

- **910 tests / 0 failures / 0 ignored**
  - 242 lib unit
  - 123 PUC official-suite files across 5 dialects (5.1 = 23,
    5.2 = 26, 5.3 = 27, 5.4 = 32, 5.5 = 15)
  - 40 end-to-end programs × 5 dialects byte-diff vs installed PUC
    binary
  - 64 method-JIT dialect-audit tests (`Value`-variant introspection)
  - 28 trace-JIT audit tests
  - 13 C API conformance tests
  - 10 sandbox embedding tests
  - 8 fast smoke tests
  - ~500 trace-JIT integration tests

### Performance

Master gate (`vs.X ≤ 0.50`, luna ≥ 2× the reference):

- **vs PUC 5.1-5.5: 35 / 35 cells PASS** across all 7 microbench
  workloads × 5 dialects
- **vs LuaJIT 2.1: 6 / 7 cells PASS**. `binary_trees_n10` lands at
  0.83× (luna 1.21× faster than LuaJIT 2.1, just shy of the 2× gate)
  — this is the design ceiling under luna's no-NaN-boxing + PUC
  bytecode-compat constraints.

See `docs/performance.md` for the full snapshot.

### Public surface (frozen for 1.x)

- Rust embedding API: `Vm`, `Value`, `LuaVersion`, the `Vm::open_*()`
  stdlib loaders, the native-function registration helpers
- Script-host sandbox pattern: see `examples/sandbox_demo.rs` and
  `tests/sandbox.rs`
- C ABI: `lua.h`-compatible subset under `src/capi.rs`, conformance
  locked by `tests/capi.rs`
- Bytecode binary compat: PUC-compiled `.luac` files load directly
  into luna for the corresponding dialect; luna's compiler emits
  matching format

### Major features

- Full dialect support — all 5 Lua versions in a single binary,
  per-`Vm` dialect selection
- Cranelift method-JIT for hot top-level chunks + cranelift
  trace-JIT for hot loop / recursive shapes
- PUC-faithful Lua semantics including: integer subtype (5.3+),
  bitwise operators (5.3+), `<const>` / `<close>` attributes (5.4+),
  `global` keyword + named varargs (5.5+), `goto` / labels (5.2+),
  full coroutine + metatable + weak-table + `__gc` finalizer
  support, generational GC pacing
- Sandbox-grade embedding: per-`Vm` instruction + memory budgets,
  bytecode-load gating, host native callbacks, no required global
  state

### Documentation

- `README.md` — overview + quick-start
- `docs/compatibility.md` — embedder compatibility surface
- `docs/performance.md` — perf snapshot
- `cargo doc --open` — full API reference

### Test environment

Tested on macOS 25.5 / aarch64 (M-series) with rustc 1.86+ and
cranelift 0.124. PUC binaries: Lua 5.1.5, 5.2.4, 5.3.6 built from
source; Lua 5.4.8, 5.5.0 + LuaJIT 2.1.1781602682 via brew.
