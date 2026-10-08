# Unsafe accounting

luna uses `unsafe` Rust in a small number of bounded categories: the GC
heap's `NonNull`-based pointer model, the interpreter's in-place access
to registers, constants and table storage, the JIT backend's FFI to
Cranelift-emitted machine code, the `lua.h`-compatible C ABI, the
AOT-binary runtime entry, and the cross-thread `feature = "send"`
SendVm newtype. Every `unsafe` block and `unsafe impl` in every crate,
tests included, carries a `SAFETY:` note that states the premise it
relies on at that site; `clippy::undocumented_unsafe_blocks` is a
workspace lint and CI fails on a block without one.

This page is the human-readable companion to `cargo-geiger`'s
machine-grepable summary (see §6). For the **embedder-surface
contract** (no `unsafe` required to use `luna-core` or `luna-jit`'s
public API) see [`security.md`](security.md) §5.

---

## 1. Snapshot (4.1 development, October 2026)

| Metric | Count | Notes |
|---|---:|---|
| `unsafe` sites in all crates (every `.rs` file under `crates/`) | **1421** | CI ceiling in `.github/workflows/ci.yml::unsafe-drift` |
| of which in tests, benches and examples | 224 | unit-test files under `src/` and the `tests/`, `benches/`, `examples/` trees |
| **`pub unsafe fn` in the public API** | **7** | six `#[doc(hidden)]`, and `MemOwner::raw`, see §5 |
| **`pub unsafe extern "C" fn`** | 200 | the C API (143), the `luna_jit_*` helpers compiled code calls (51, re-exported by `luna-jit`), the AOT entries (4) and two in tests; see §5 |
| **`unsafe impl Send` / `Sync`** | 10 | see §5 |

A "site" is a line matching `unsafe (\{|fn |impl |trait |extern )`,
the pattern CI counts (§4). Several `unsafe` blocks on one line count
once; a block spread over several lines counts once; a comment that
quotes the pattern counts too.

## 2. Distribution by crate and module

| Crate | Module | Sites | What the `unsafe` is for |
|---|---|---:|---|
| `luna-core` | `vm/exec` fast loop (`fast.rs`, `fast/*`, `fast_arith.rs`) | 61 | reading and writing registers and constants in place through the frame's register window; the running frame pointer; instruction fetch |
| | `vm/exec/index_*` | 38 | table reads and writes the loop finishes itself with the operands read in place; the `__index` / `__newindex` miss paths entered with raw operand pointers |
| | `vm/exec` (other) | 84 | `Gc` handle mutation, frame and stack bookkeeping, coroutine resume, trace entry and exit register copies, the runtime entry points compiled code calls, and what the C API needs from the VM (`host_c`: a thread's C stack and state, C userdata blocks, a continuation's frame) |
| | `runtime/heap*`, `gc_ptr.rs` | 84 | the intrusive mark-sweep heap: allocation, marking, sweeping, finalisation, the `Gc<T>` handle, the debug check of the slow-store bit |
| | `runtime/table*` | 48 | the table's raw layout: the node array, the slab-backed array part, tag-driven marking |
| | `runtime/mem` | 57 | the allocation context and the containers whose blocks come from it (§3.8): raw blocks from the system allocator or the host's `lua_Alloc`, the vector's and boxed slice's initialised prefix |
| | `runtime` (other) | 38 | string headers and their trailing bytes, the value tag/payload encoding, closure upvalue storage |
| | `vm/lib_*` | 55 | `Gc` handle mutation in the standard library (io handles, `table`, `debug`), the table writes that build each library, and on Windows the `ReadFile` call that reads a console as the MSVC C library does, the `CreateFileW`, `DeleteFileW` and `MoveFileExW` calls that open, remove and rename files as it does, the `MultiByteToWideChar` and `WideCharToMultiByte` calls that take names and environment text through the ANSI code page as it does, and the `File` made of the standard input handle for seeking it |
| | `vm` (other) | 48 | userdata trampolines, typed natives, SendVm, async natives, call-stack walks |
| | `stdio.rs` | 1 | C-style standard output writing descriptor 1 without closing it |
| | `native_stack.rs` | 8 | reading the running thread's stack bounds from the OS (`pthread_getattr_np`, `pthread_get_stackaddr_np`, `GetCurrentThreadStackLimits`, and on glibc's main thread `__libc_stack_end` and `getrlimit`) |
| | `jit`, `frontend` | 11 | trace metadata handed to the backend; interned-name text |
| | unit-test files under `src/` | 20 | tests that inspect raw layouts; a test `lua_Alloc` |
| | `tests/` | 54 | integration tests: a poisoning global allocator, async wakers, userdata internals, a raw write into a read-only table, the host C library's `%p`, a counting `lua_Alloc`, the environment variables of the Windows file-name test |
| `luna-jit` | `capi*` | 382 | the C API: raw `lua_State` pointers, C strings, `lua_Debug` and `luaL_Buffer` structs and C function pointers across the boundary (§3.7) |
| | `jit_backend` | 59 | executable code memory (including the baseline trace tier's code pages), compiled-function entry points (the LLVM backend's trace entries among them), `Send` for handles that own JIT modules or code pages, copying compiled code out to share it between Vms, the debug dump of a trace's machine code |
| | other | 2 | the CLI's `arg` table and the `lua_facade` table handle |
| | unit-test files under `src/` | 70 | tests that call compiled code or the `extern "C"` helpers directly |
| | `tests/`, `benches/`, `examples/` | 43 | a C API state driven from Rust, a counting global allocator, the `send` overhead bench |
| `luna-jit-helpers` | | 168 | the `luna_jit_*` `extern "C"` helpers compiled code calls (§3.5) |
| `luna-jit-llvm` | `src/` | 6 | LLVM execution engines (one per compiled method or trace), `Send` for an engine compiled on the background compile thread, and the register-file GEPs |
| | `tests/` | 34 | calling LLVM-compiled chunks |
| `luna-runtime-helpers` | | 45 | the AOT binary's C entries, the linker-section walkers (§3.6), the PE header walk on Windows, the helper link anchor |
| `luna-aot` | | 3 | the embedded bytecode section of an AOT binary |
| `llvm-jit-probe` | | 2 | the LLVM toolchain probe |
| `luna-jit-derive`, `luna-tools`, `luna-fuzz` | | 0 | |
| **Total** | | **1421** | |

## 3. Pattern catalog

### 3.1 `Gc<T>` handles

luna's GC handles are `Gc<T> = NonNull<T>` over an intrusive mark-sweep
heap. A shared read goes through `Deref`; a write goes through
`Gc::as_mut`, which is `unsafe`. The contract is stated at the top of
`runtime/heap.rs`: the runtime is single-threaded, and a `Gc` pointer
is valid until a `collect()` that does not reach it from the roots.
This holds because `Vm` is `!Send + !Sync` by default, and the Vm's
root set covers every reachable handle (host roots, globals, stack,
frames, metatables, hooks, the running coroutine). For cross-thread
use see `SendVm` (§5).

Only the heap makes handles: `Gc::from_ptr` is an `unsafe fn` whose
caller vouches that the pointer is a live object the heap manages, so a
safe function that takes a `Gc` can read through it. Code that needs an
object's GC header (the write barriers, the marker) takes the handle
and gets the header from it; the sealed `GcObject` trait, implemented
only for the runtime's object types, guarantees each of them is
`#[repr(C)]` with the header first (checked at compile time).

### 3.2 In-place register and value access (interpreter fast loop)

The interpreter's fast loop keeps the running frame's register window,
constants and instruction pointer as raw pointers in locals, as PUC Lua
keeps `base`, `k` and `pc` in registers. An arm reads a value's tag
byte and payload word where they are instead of copying the 16-byte
`Value` out: a whole-value load right after a split store cannot be
forwarded from the store buffer and waits for the cache. The premises
are always the same few, each written once:

- a register index decoded from an instruction is below the proto's
  `max_stack`, and the frame's window was sized to `base + max_stack`
  when the frame was pushed; constant indices are inside the proto's
  constants (the compiler and the bytecode verifier keep both in range);
- a payload is read only as the type its tag names (`Value` is
  `#[repr(C, u8)]`: the tag is the first byte, the payload the second
  word);
- `fr` points at the running frame, the top of `frames`, and is taken
  again after anything that can push or pop a frame.

The pc store, the instruction fetch, register reads and writes and the
truth test are single macros in `vm/exec/fast/step.rs`; an arm reads
all its operands in one block and leaves the slow path, which may run
Lua code, outside it.

### 3.3 Table internals

A table keeps its hash part as a raw node array (`nodes`,
`node_mask`) and its array part as a tag array and a payload array in
one slab, inline for small tables. Reads walk the node chain by index
(`next` links are node indices written by `insert_new`), and the GC
marks array slots by tag without building values. The slab is
allocated, grown and freed with `std::alloc`; the free lives in one
place (`Table::free_array_slab`).

### 3.4 Unchecked frame and instruction access

On a few paths the bounds are established right before the access and
the check is measurable in the interpreter's instruction count: the
loop head's top-frame read (`unwrap_unchecked` on a non-empty frame
stack), the instruction fetch, and the `unreachable_unchecked` arm of a
match on a frame known to be a Lua frame. Where the optimiser can see
the bound, safe indexing is used instead (the frame pop). The frame
below a returning one is reached through a raw element pointer: a
mutable index would reborrow every frame, the running one too, whose
pointer the fast loop still writes through.

### 3.5 JIT and C ABI

Compiled code (Cranelift, LLVM or AOT) calls back into Rust through the
`luna_jit_*` `extern "C"` helpers in `luna-jit-helpers`. Each helper's
`# Safety` section is its contract with the code generator: it is
called inside an `enter_jit` window on the calling thread (opened with
the running closure when the helper reads upvalues), and its pointer
arguments are what the section names (a live table, an interned string
key, a value's tag with its payload, a writable out slot). Inside the
window the thread-local `JIT_VM` holds the `Vm` the dispatcher lent to
the compiled call, which the dispatcher does not touch until the call
returns. The symbols are `#[unsafe(no_mangle)]`; only this crate
defines `luna_jit_` names. The helpers turn their raw arguments into
handles, values and references themselves, so the `Vm` methods they
call (`jit_spill_stack`, `jit_op_tforcall`, the string accumulator)
have safe signatures. `IntChunkCompiler::enter` and the guard that
restores the thread-locals only store pointers; the dereference happens
in the helpers, under the contract of whoever ran the compiled code.

The C API (`luna-jit/src/capi*`) receives raw `lua_State` pointers and
C strings across the ABI boundary. Each `lua_*` function's `# Safety`
section says what it needs: a state from `luaL_newstate` that
`lua_close` has not freed and that no other call is using (a C function
the state is running may call back in), and string arguments that are
null or NUL-terminated. `Box::into_raw` / `Box::from_raw`
pairs move trace metadata between Cranelift's symbol table and the
trace cache; each `into_raw` has one matching `from_raw` on eviction.
The LLVM backend keeps each `(Context, ExecutionEngine)` pair by value
in its storage, the engine declared before the boxed context so it
drops first.

### 3.7 The C API's error boundary

An error raised in a C function leaves it with `longjmp` to a `setjmp`
boundary set up in C just before luna called it (`csrc/shim_core.c`). The
API functions that raise are C functions that call their Rust half,
which reports the error after it has returned, so a jump never crosses a
Rust frame; their exported names are naked Rust functions holding only a
jump to the C function, with no frame of their own. Before a C function
runs, the state's Vm pointer is replaced by one derived from the caller's
`&mut Vm` and put back after, so C reaches the Vm only through the
reference held by the Rust code that called it.

### 3.6 AOT binaries

An AOT binary finds its embedded bytecode, trace metadata, string keys,
inline frame chains and the prototype slots of inlined calls in linker
sections, bracketed by
`__start_` / `__stop_` symbols (ELF), `section$start` / `section$end`
(Mach-O), or located by walking the PE headers (Windows). The walkers
are `unsafe fn`s whose contract is the section layout `luna-aot` emits;
they turn the section into a slice of index entries once and keep the
`unsafe` to the reads of each entry's payload and the write of its
slot. The PE walk computes addresses with wrapping arithmetic and reads
only header bytes inside the first pages the loader maps.

### 3.8 The allocation context and its containers

Every block a Vm allocates comes from its allocation context
(`runtime/mem`), so that a host's `lua_Alloc` or a `MemoryPolicy` sees it
and a refused allocation becomes an error instead of the end of the
process. Stable Rust's `Vec` and `Box` cannot take an allocator, so the
module has its own: `LVec`, `LSlice`, `LBox` and the type-erased `LAny`
own a block and its length, and free it through the handle they keep. The premises: a block
is freed once, with the layout (or, for a host function, the size) it was
allocated with, by the context that allocated it, which its owners keep
alive past every container; a container's first `len` slots are
initialised; a host function follows PUC's `lua_Alloc` contract, which the
`unsafe` constructor `MemOwner::raw` makes its caller vouch for. Collected
objects take their block from the context too and are freed by the sweep
through it.

## 4. CI enforcement

Two CI checks cover `unsafe`:

- the `unsafe-drift` job in `.github/workflows/ci.yml` counts the sites
  in every `.rs` file under `crates/` on every push, `luna-jit-llvm`
  included although CI does not build it, and fails above the ceiling.
  The ceiling is the exact count, **1290**, with no headroom;
- `clippy::undocumented_unsafe_blocks` is set in the workspace
  `[lints.clippy]` table, and the lint job runs clippy with
  `-D warnings`, so a block or `unsafe impl` without a `SAFETY:` note
  fails the build.

A change that adds `unsafe`:

1. puts a `SAFETY:` note on the block that states its premise at that
   site, and a `# Safety` section on any `unsafe fn`;
2. raises the ceiling in `ci.yml` to the new exact count and says why
   in the commit message, or removes an equivalent site elsewhere.

The ceiling is never raised to a round number to make room.

History: 461 sites at v1.1, 490 at v1.3 (ceiling 490), 470 at the
start of October 2026, after 4.0.0, all counted in `luna-core` and
`luna-jit` `src/` only.
The October 2026 interpreter performance work raised that count to 613
(mostly §3.2 and §3.3, plus unit tests moved out of `jit_backend` into
files of their own, which moved sites without adding any). An audit of
every site added in that work then replaced the ones a safe form covers
at no measured cost (pointer arithmetic with `wrapping_add`, indexing
whose bound the optimiser sees, `Vec::truncate`), folded blocks that
share a premise into one, made the table miss paths that take raw
pointers `unsafe fn`, kept `Value::pack_into` crate-private, and set
the ceiling to the resulting 575.

A second audit then extended the count to every crate (981 sites in
all) and went through each of them: blocks that cited a shared note
now state their own premise, test code has notes, the `luna_jit_*`
helpers have their contracts written down, the AOT section walkers
that dereferenced the pointers they were given from safe functions
became `unsafe fn`s, and reads a safe form covers (through the `Gc`
handle, `as_bytes`, `downcast`, `table_of`, a compiled chunk's
`call_with`) replaced their blocks. The count went to 862; `luna-core`
and `luna-jit` `src/` held 563 of them.

That audit left 17 safe functions that dereferenced raw pointers their
callers passed in (`Gc::from_ptr`, the write barriers, the marker, the
`Vm` methods the JIT helpers call, `IntChunkCompiler::enter`). They now
take handles or references, or became `unsafe fn`s where the pointer
cannot be typed (`Gc::from_ptr`, the marker's raw-header entry, the
heap's object linking and the string table's removal). Each call that
builds a handle from a raw pointer is now a block with its own note,
mostly in the `luna_jit_*` helpers. The write barriers keep one
non-generic `unsafe fn` body behind their generic entry points: a
generic body changed how LTO compiled `Table::resize`, which cost
every table rehash about 700 instructions. That brought the count to
908.

The baseline trace tier added 13: its code pages (copying code in and
protecting it, freeing them with the `Vm`'s code, and `Send` for the
arena and for the pool of reused pages), the entries it hands out and
the optimizing tier's entry that replaces one (a pointer turned into a
`TraceFn`, and the parent trace's exit cell that is pointed at it), the
C math functions its code calls, and two blocks in the unit tests that
run each primitive. That is 921.

Rooting the values a trace holds only in registers while it calls a
helper that can collect added 3, all in `luna-jit-helpers`: the
function that reads the root list compiled code passes (a count word
followed by tag and payload pairs) and its two blocks, one viewing the
words as a slice and one packing each pair into a value. That is 924.

Inlining calls into functions of other prototypes added 12. In
`luna-jit-helpers` 7: the method lookup through `__index` tables and the
upvalue read of an inlined function's own closure (each an `extern "C"`
helper with a block for its raw arguments and one for the checked read),
and the frame-materialise helper turning each frame's closure payload
back into a handle. In `luna-runtime-helpers` 5: the deploy-side walker
of the prototype-slot section (its bracket symbols on ELF and Mach-O,
the slice of index entries, the read of each 16-byte hash and the write
of its slot). That is 936.

Running an AOT binary's script on a `Vm` of the dialect it was compiled
for added 2, both in `luna-runtime-helpers`: the C entry that takes the
dialect (`luna_aot_run_dialect`), and the block in the old two-argument
entry `luna_aot_run` that calls it for 5.5. That is 938.

Sharing compiled code between the Vms of an engine added 10. In
`luna-jit` 7: copying a compiled function's bytes out of the code
memory that holds it (an `unsafe fn` and its block, plus its two
callers when a trace and when the optimizing tier's code is shared),
the same read for a method-JIT function, and the entries of code a Vm
installed from the engine, one for a trace and one for the optimizing
tier's code taken at tier-up (a pointer turned into a `TraceFn`). In
`luna-core` 1: dropping the chunks the collector found dead from the
list a Vm keeps to find inlined functions by content, which reads the
header of each listed prototype before the sweep. In a unit test 2
that runs a trace's code before and after its addresses are rewritten
(calling the code, and copying the Cranelift function out). That is
948.

Read-only tables added 4: `Vm::set_readonly`, which sets the mark through
the table's `Gc` handle; the debug check that walks the heap's lists and
reads each header's slow-store bit (two blocks); and the integration test
that calls `Table::set` on a read-only table directly. That is 952.

`LUNA_TRACE_CODE_DUMP`, which copies each Cranelift trace's machine code to
a file, added 1: the read of the finalized or placed code. That is 953.

The C API's message handlers and errors added 2 in `capi.rs`:
`lua_enablereadonlytable` (Redis's call that marks a table read-only) and
its read of the state. Their tests added 22: `capi_pcall_handler.rs` drives
`lua_pcall` with Lua and C message handlers, global metamethods and a
read-only `_G` in every dialect (19), and `pointer_text.rs` asks the host
C library's `snprintf` for its `%p` (3). The `luna` command's C-style
standard output writes descriptor 1 directly on Unix (1, `stdio.rs`).
That is 978.

The complete C API (every function of `lua.h`, `lauxlib.h` and
`lualib.h` of the five versions) brought `capi*` from 71 to 376: each
exported function is a `pub unsafe extern "C" fn` and makes its call
context from the raw `lua_State` in one `unsafe` block (`Api::new`), and
the functions that take C strings, `lua_Debug` / `luaL_Buffer` structs,
readers, writers, hooks or continuations read or call them in one more.
luna-core's `host_c` added 15 (a thread's C stack, a userdata's raw block,
the light C functions), and `coro.rs` 1. The Rust-driven C API tests
(`capi.rs`, `capi_pcall_handler.rs`, 35 sites) were replaced by C host
programs compiled against PUC and luna; one site came back in
`jit_storage_mismatch_no_abort.rs`, which declares two C API functions
written in C. That is 1269. Unit tests that drive the C API's Rust half from Rust (`capi/unit_tests.rs`) added 21. That is 1290.

Sizing tables as each PUC version does, so that `#t` finds the same
border, added 4. In `luna-core` 1: `Heap::new_table_presized`, which
sizes the table it just made through its handle. In `luna-jit-helpers`
3: `luna_jit_table_reserve_list`, the `extern "C"` helper that grows a
constructor table's array part before compiled code stores its list
items, with a block for the Vm and one for the table. That is 1374. The LLVM backend's background compile thread added 1 (`Send` for its engine): 1375.

Traces inlining vararg callees and making closures in inlined frames
added two helpers (`luna_jit_op_closure_in`, `luna_jit_set_top`, in
`luna-jit-helpers/src/inlined.rs`): each is a `pub unsafe extern "C" fn`,
and the bodies take the current Vm (two blocks) and the inlined frame's
closure from its payload (one), +5; a side trace started inside an
inlined frame is entered at an offset into the parent's registers, which
replaced the old entry call one for one. That is 1380.

The MSVC C library's `FILE` (`vm/lib_io/msvc*`) is plain safe code over
the file; reaching it from a file handle takes the same two `Gc` borrows
as the other io paths, and reading a Windows console with `ReadFile`
takes the declaration and the call. The handle-state code it replaced had
8, so the count went down by 4, to 1376.

Raising "stack overflow" instead of overflowing the native stack added
20 to the 1386 the table above reached: `native_stack.rs` in luna-core 8 (one foreign block and one call for
each of Linux / Android, the Apple targets and Windows, reading the
thread's stack bounds, and one more of each for glibc's main thread,
whose bounds are read without the stream `pthread_getattr_np` opens),
`recursion.rs` in luna-jit-helpers 11 (the helpers that fill a
compiled function's self-call context, count the LLVM tier's native self
calls, and make a self call in the interpreter when the native stack or
the call budget runs out), and `jit_handle.rs` in luna-jit 1 (freeing
the code arena that holds a self-recursive function's ring of body
copies). That is 1406.

Opening, removing and renaming files on Windows through the Win32 calls
the MSVC C library makes (`vm/lib_io/winfs.rs`), so that a failure gives
the library's error, added 5: the foreign block, the three calls, and
taking ownership of the handle `CreateFileW` returns. That is 1411.

Taking file names, command lines and environment text through the ANSI
code page as that library's narrow functions do (`vm/lib_io/winfs.rs`:
one call each way), seeking standard input through its handle
(`vm/lib_io/crt.rs`), and the environment variables the Windows
file-name test sets (`tests/win_names.rs`) added 4. That is 1415.

Reaching the caller's frame in the fast return (`vm/exec/call_fast.rs`)
through a raw element pointer instead of a mutable index over all the
frames, which invalidated the pointer to the running frame, added 1.
That is 1416.

The table read and the table write of the fast loop for a 5.2 / 5.3
upvalue table indexed by a key that is not a string constant
(`GetTabUpR`, and `SetTabUpR` / `SetTabUpK` through one arm) probe the
table in place as the other table arms do, and added 2. That is 1418.

The fast loop's arm for 5.5's numeric `for` step, which keeps its index
in the loop variable, and its arms for 5.1's `GetGlobal` / `SetGlobal`
(a global named by a constant past 255) read their operands in place as
the other arms do, and added 3. That is 1421, the ceiling now.

## 5. Public `unsafe` surface

### `pub unsafe fn` (7, six of them `#[doc(hidden)]`)

| Location | Function | Why |
|---|---|---|
| `runtime/gc_ptr.rs` | `Gc::<T>::from_ptr` | Rebuilds a handle from a pointer compiled code passes around; the pointer must be a live object. |
| `runtime/gc_ptr.rs` | `Gc::<T>::as_mut` | Internal mutation; embedders use `TableBuilder` / `LuaUserdata`. |
| `runtime/value.rs` | `Value::as_closure_unchecked` | JIT hot path; skips the tag match. Safe alternative: match `Value::Closure(_)`. |
| `runtime/value.rs` | `Value::as_int_unchecked` | Same shape. |
| `runtime/value_raw.rs` | `Value::pack` | Low-level constructor from the array-part encoding, used by the JIT and the C API. |
| `runtime/mem/ctx.rs` | `MemCtx::set_raw_alloc` | `lua_setallocf`: the C API swaps the host function, which must accept the previous one's blocks. |
| `runtime/mem/ctx.rs` | `MemOwner::raw` | A host that supplies a `lua_Alloc`-style function vouches for it; the safe way to watch or limit a Vm's memory is `MemOwner::policy`. |

None of these but `MemOwner::raw` appears in the `cargo doc` view of the
API; using the API does not need any of them.

### `pub unsafe extern "C" fn` (203)

| Location | Count | Why |
|---|---:|---|
| `luna-jit/src/capi*` | 143 | the C API, called from C with raw `lua_State` pointers (the functions written in C are reached through naked jumps, which are not `unsafe fn`s) |
| `luna-jit-helpers/src/*` | 54 | the `luna_jit_*` helpers compiled code calls (§3.5); each has a `# Safety` section |
| `luna-runtime-helpers/src/*` | 4 | `luna_aot_run_dialect`, the AOT binary's entry, called by the generated C `main` with the dialect the script was compiled for; `luna_aot_run`, the same entry for 5.5 |

### `unsafe impl Send` / `Sync` (10)

| Location | Impl | Why |
|---|---|---|
| `runtime/function.rs` | `LuaClosure: Send + Sync` | The closure type holds no `Rc` / `RefCell`; `Gc<LuaClosure>` stays `!Send`. |
| `runtime/table.rs` | `Table: Send + Sync` | Same shape. |
| `vm/send_vm.rs` | `SendVm: Send` | `feature = "send"`: every method takes the `RwLock` write guard before forming `&mut Vm`, so the `Vm` keeps one mutator at a time; `SendVm` is deliberately not `Sync` (see [`threading.md`](threading.md)). |
| `jit_backend/jit_handle.rs` | `JitHandle: Send` | Owns its `JITModule` (through `SendJitModule`) by value together with the entry pointer into that module's code, so the two move between threads together. |
| `jit_backend/trace/compile.rs` | `TraceHandle: Send` | Required by the thread-local trace cache; the cache is only touched from its own thread. |
| `jit_backend/trace/lir/code.rs` | `CodeArena: Send` | Owns the baseline tier's code pages of one `Vm`; the code in them only runs on the thread that owns that `Vm`. |
| `jit_backend/trace/lir/code.rs` | `Spare: Send` | A freed chunk of code pages, writable again, that nothing points into; the pool hands each to one arena at a time. |
| `luna-runtime-helpers` `jit_helpers_pin.rs` | `PinnedFn: Sync` | The elements of an immutable static of helper addresses, never written through or dereferenced. |

Test and bench code adds three more `unsafe impl`s that are not part of
any library: two `GlobalAlloc` wrappers over `System` (a poisoning one
in `luna-core/tests/gc_stress_poison.rs`, a counting one in
`luna-jit/tests/jit_code_freed_with_vm.rs`) and `Send` for the no-op
wrapper in `luna-jit/benches/bench_send_overhead.rs`.

## 6. Reproducing the counts

```sh
grep -rE --include='*.rs' 'unsafe (\{|fn |impl |trait |extern )' crates | wc -l
# 1290, the ceiling in ci.yml's unsafe-drift job
cargo clippy --workspace --all-targets \
    --exclude llvm-jit-probe --exclude luna-jit-llvm -- -D warnings
# no undocumented_unsafe_blocks warnings (the LLVM crates need
# LLVM_SYS_181_PREFIX and are checked the same way where LLVM 18 is installed)
```

`cargo-geiger` gives the per-crate breakdown by kind (expressions,
functions, impls, traits) and the supply chain's counts:

```sh
cargo install --locked cargo-geiger
cargo geiger -p luna-jit --bin luna
```

luna-core has no third-party dependencies, so its `unsafe` footprint
is entirely first-party.

## 7. See also

- [`security.md`](security.md) — embedder-surface contract and threat
  model
- [`architecture.md`](architecture.md) §3 — where the `unsafe`-dense
  layers sit
- [`threading.md`](threading.md) — the `SendVm` rationale

---

*Counts measured with the §6 grep on the 4.1 development branch,
October 2026. The CI ceiling is the load-bearing number; this page
explains why the sites exist.*
