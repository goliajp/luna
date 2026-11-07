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
| `unsafe` sites in all crates (every `.rs` file under `crates/`) | **1907** | CI ceiling in `.github/workflows/ci.yml::unsafe-drift` |
| of which in tests, benches and examples | 264 | files under a `tests/`, `benches/` or `examples/` directory, or whose name contains `tests` |
| **`pub unsafe fn` in the public API** | **1** | `MemOwner::raw`; thirty-three more in `luna_core::__private`, which is not public API, see §5 |
| **`pub unsafe extern "C" fn`** | 208 | the C API (143), the `luna_jit_*` helpers compiled code calls (59, re-exported by `luna-jit`), the AOT entries (6); see §5 |
| **`unsafe impl Send` / `Sync`** | 13 | see §5 |

A "site" is a line matching `unsafe (\{|fn |impl |trait |extern )`,
the pattern CI counts (§4). Several `unsafe` blocks on one line count
once; a block spread over several lines counts once; a comment that
quotes the pattern counts too.

Changes since the 1801 snapshot (net 108 more):

- the stores into a table go through the table's handle (`Gc<Table>::set`,
  `set_int`, `try_set_existing`, `set_metatable`), which borrows the table
  in one block (`table/store.rs`) and takes the write barrier on the
  handle; the callers' blocks that borrowed the table for those stores are
  gone (44 fewer)
- `__private::TableInternals::{set_unguarded, set_int_unguarded}` are
  `unsafe fn` (declaration and implementation, four sites): they skip
  the read-only test and the write barrier, which their callers, the
  JIT's store helpers, make themselves
- those stores and `Heap`'s write barriers are crate-internal: given
  another heap than the one that made the object, they could free objects
  the VM still uses. luna's other crates reach them as the `unsafe fn`s of
  `__private::{TableStores, HeapBarriers}` (fourteen sites, declarations
  and implementations), and the calls in the C API (15), the JIT's helpers
  (9, one of them a helper that is now an `unsafe fn` itself) and a test
  (1) are blocks that state whose heap the object is in; `table/store.rs`
  adds a test of a read-only table (1)
- a JIT test sets a metatable through `Vm::set_metatable` instead of a
  block (1 fewer)
- `__private::new_heap` is an `unsafe fn` (1): a heap of one's own can be
  dropped while handles to its objects live on; the tests, luna-aot and
  luna-fuzz that make one call it in a block (16)
- `__private::VmHeap::heap_mut` is an `unsafe fn` (declaration, implementation
  and a doc example's block, 3): a `&mut Heap` lets the VM's heap be swapped
  for another VM's. The calls that were not in a block yet, the JIT's table
  allocations (4) and the AOT key resolver's intern (1), are blocks now
- a JIT backend is an `unsafe impl` of `IntChunkCompiler` and
  `TraceCompiler`, which are `unsafe trait`s (2), for the null backend,
  Cranelift, LLVM, luna-jit-llvm, the AOT harvester and a test's compiler
  (10); a compiled trace is made by `unsafe fn CompiledTrace::from_parts`
  (1) and `from_aot_meta` (2, with the block that calls `from_parts`) and
  installed through the `unsafe fn`s `__private::install_aot_trace` and
  `ProtoTraceInternals::set_jit_next` (3); their calls are blocks (luna-jit's
  compile and adopt paths, the LLVM method JIT's pending entry, the AOT
  resolver, and tests: 11); doc examples of what no longer compiles (3); two
  comments that quoted the pattern are gone (2 fewer)
- Vms given clones of one `MemOwner` each allocate through a context of
  their own on a shared budget: a context reads its budget through a
  pointer (1), and the last context using a budget frees it (1)
- the JIT state and the tier-up source left safe reach:
  `__private::VmJitState::jit_mut` is an `unsafe fn` (declaration,
  implementation and a doc example's block, 3), whose remaining callers in
  the JIT's helpers are blocks (3); `__private::put_tier_source` is an
  `unsafe fn` (1), called by the LLVM tier (2); the doc examples of a
  compiled trace show it (2)
- the helpers that take raw register words are `unsafe fn`s:
  `__private::VmJitFrames::jit_concat_regs` and
  `__private::VmJitAlloc::jit_gc_step` (declaration, implementation, the
  block forwarding to the crate function, and a doc example each, 8), the
  crate functions behind them (2); the callers' blocks absorbed the calls
- containers are made on a context only inside luna: `__private`'s
  `lvec_new`, `lvec_with_capacity`, `lbox_new` and `lany_new` are
  `unsafe fn`s (4), called by the C API in blocks (3);
  `__private::VmJitAlloc::jit_str_buf_acquire` is an `unsafe fn` (2)
- the JIT storage is no longer handed out safely: the tests that compile a
  trace into a Vm's storage borrow it through `jit_mut` in a helper (2)
- with the `send` feature a budget reads the host's allocation function
  without a lock: the function and its `ud` sit in a block of their own,
  swapped in whole and freed with the budget (2: the blocks it replaced
  are kept by address and freed together when the budget goes); a test
  swaps the function from another thread while a Vm allocates (2)
- `JitStorage` is an `unsafe trait` (1): the Vm clears a storage's cache
  from safe code, so a storage vouches that clearing it leaves its code
  mapped; the null, Cranelift and LLVM storages are `unsafe impl`s (3),
  and a doc example shows one (1)
- the optimizing tier compiles a looping trace as two functions, the
  first pass and the loop, and the baseline code's back edge calls the
  loop through the trace's loop cell: `__private::set_tier_up_loop`, which
  writes that cell, is an `unsafe fn` (1), as is the backend's `set_loop`,
  which calls it (2, with its block); the Cranelift tier-up calls
  `set_loop` for a trace compiled now and for one taken from a shared
  image (2) and copies the loop's code out for the image (1); the LLVM
  tier calls it for its own loop (1); a Vm adopting a shared image writes
  the image's loop into the cell (1). The block that writes a parent
  trace's exit cell at tier-up moved from `dispatch.rs` to `counting.rs`
  (no change)
- the slice of a trace's register buffer a collection step run in place
  reads is made where the step runs (`jit_gc_step`, `vm/exec`) instead of
  in the helper that calls it (`luna-jit-helpers`): one block moved (no
  change in total)
- a side trace's entry is no longer written into a cell of its parent's
  exit, which no code read: the block in `counting.rs` that rewrote that
  cell when the side trace moved to the optimizing tier is gone (1 fewer)
- a test runs two traces made from parts by hand, one leaving the words
  past its window as a trace counting there does, to check that a trace
  not counting there never reads them: their code (an `unsafe extern "C"
  fn` and its block) and the block that makes them (3)

## 2. Distribution by crate and module

| Crate | Module | Sites | What the `unsafe` is for |
|---|---|---:|---|
| `luna-core` | `vm/exec` fast loop (`fast.rs`, `fast/*`, `num.rs`, `num/words.rs`) | 85 | reading and writing registers and constants in place through the frame's register window; the running frame pointer; instruction fetch; a numeric `for`'s control values read a word pair at a time (aarch64) |
| | `vm/exec/index*` | 41 | table reads and writes the loop finishes itself with the operands read in place; the `__index` / `__newindex` miss paths entered with raw operand pointers |
| | `vm/exec` (other) | 96 | `Gc` handle mutation, frame and stack bookkeeping, coroutine resume, trace entry and exit register copies, the runtime entry points compiled code calls, and what the C API needs from the VM (`host_c`: a thread's C stack and state, C userdata blocks, a continuation's frame) |
| | `runtime/heap*`, `runtime/gc_hooks` | 176 | the intrusive mark-sweep heap: allocation, marking, sweeping, finalisation (the lists of objects with finalizers and of those due), the `Gc<T>` handle, the debug check of the slow-store bit; the collector checks of the test-only builds (`gc-verify`: the freed-address probes, the tricolour and string-count checks) |
| | `runtime/puc` | 138 | the model of PUC's allocations: the records it keeps over raw object headers (the parser's tables and frames a collection step marks, 5.1's environment records, the objects PUC makes and luna does not yet, the strings a load sets aside), its marking and sweeping of them, the 5.5 sizer |
| | `__private` (`luna_core::__private`) | 64 | the internals luna's own crates use: the `unsafe fn`s among them (a `Gc` from a pointer, its exclusive borrow, a value from its array encoding, the host's allocation function, the two table stores that leave the read-only test and the write barrier to the JIT's helpers, the table stores and write barriers that must be given the object's own heap, the VM's heap and the JIT state borrowed mutably, the tier-up source put back on a trace, the optimizing tier's loop function written into a trace's loop cell, the method entry a background compile hands over, the containers made on a context, the JIT entries that take register words (`jit_concat_regs`, `jit_gc_step`), and the JIT entries that trust compiled code's frame layout) and the blocks that forward to them |
| | `runtime/table*` | 54 | the table's raw layout: the node array, the slab-backed array part, tag-driven marking |
| | `runtime/mem` | 93 | the allocation context and the containers whose blocks come from it (§3.8): raw blocks from the system allocator or the host's `lua_Alloc`, the freed blocks a system-allocator context keeps for reuse, the vector's and boxed slice's initialised prefix, a push into room the caller made, the host's allocation function read without a lock (`send`) |
| | `runtime` (other) | 43 | string headers and their trailing bytes, the value tag/payload encoding (and its copy as a tag byte and a payload word between references), closure upvalue storage |
| | `vm/lib_*` | 32 | `Gc` handle mutation in the standard library (io handles, `table`, `debug`), the table writes that build each library, and on Windows the `ReadFile` call that reads a console as the MSVC C library does, the `CreateFileW`, `DeleteFileW` and `MoveFileExW` calls that open, remove and rename files as it does, the `MultiByteToWideChar` and `WideCharToMultiByte` calls that take names and environment text through the ANSI code page as it does, and the `File` made of the standard input handle for seeking it |
| | `vm` (other) | 38 | userdata trampolines, typed natives, SendVm, async natives, call-stack walks |
| | `stdio.rs` | 1 | C-style standard output writing descriptor 1 without closing it |
| | `native_stack.rs` | 12 | reading the stack pointer register (x86_64, aarch64); reading the running thread's stack bounds from the OS (`pthread_getattr_np`, `pthread_get_stackaddr_np`, `GetCurrentThreadStackLimits`, and on glibc's and musl's main thread `getrlimit`, on glibc also `__libc_stack_end`) |
| | `jit`, `frontend` | 26 | trace metadata handed to the backend; the backend traits a backend implements with `unsafe impl`, the constructors of a compiled trace from its parts, and the doc examples of both; the `isb` that lets a thread run code another thread wrote (aarch64); interned-name text |
| | unit-test files under `src/` | 19 | tests that inspect raw layouts; a test `lua_Alloc` |
| | `tests/` | 80 | integration tests: a poisoning global allocator, async wakers, userdata internals, a raw write into a read-only table, the host C library's `%p`, a counting `lua_Alloc` (swapped from another thread too), the environment variables of the Windows file-name test |
| `luna-jit` | `capi*` (not `capi/unit_tests.rs`) | 404 | the C API: raw `lua_State` pointers, C strings, `lua_Debug` and `luaL_Buffer` structs and C function pointers across the boundary (§3.7) |
| | `jit_backend` (not its test files) | 79 | executable code memory (including the baseline trace tier's code pages), compiled-function entry points (the LLVM backend's trace entries among them), `Send` for handles that own JIT modules or code pages, `Send` and `Sync` for the record a baseline trace keeps to be lowered again for the optimizing tier, copying compiled code out to share it between Vms (the optimizing tier's loop function as well as its first pass), writing the optimizing tier's loop function into the trace's loop cell, which the baseline code reads at its back edge, the debug dump of a trace's machine code |
| | unit-test files under `src/` (`capi/unit_tests.rs`, `jit_backend/chunk_tests_basic.rs`, `jit_backend/trace/tests/`) | 70 | tests that call compiled code or the `extern "C"` helpers directly |
| | `tests/`, `benches/`, `examples/` | 57 | a C API state driven from Rust, a counting global allocator, the `send` overhead bench |
| `luna-jit-helpers` | | 196 | the `luna_jit_*` `extern "C"` helpers compiled code calls (§3.5) |
| `luna-jit-llvm` | `src/` | 11 | LLVM execution engines (one per compiled method or trace), `Send` for an engine compiled on the background compile thread, and the register-file GEPs |
| | `tests/` | 34 | calling LLVM-compiled chunks |
| `luna-runtime-helpers` | | 48 | the AOT binary's C entries, the linker-section walkers (§3.6), the PE header walk on Windows, the helper link anchor; 2 in its test of the exported helper symbols |
| `luna-aot` | | 7 | the embedded bytecode section of an AOT binary, the heaps it compiles a script's bytecode in, and the trace compiler it wraps to harvest recordings; 2 in tests |
| `llvm-jit-probe` | | 2 | the LLVM toolchain probe |
| `luna-fuzz` | | 1 | the heap the dump reader's target loads into |
| `luna-jit-derive`, `luna-tools` | | 0 | |
| **Total** | | **1907** | |

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

A safe function never reads through a raw object pointer its caller
passes: it takes a `Gc` (an object's header as `Gc<GcHeader>`, from
`Gc::hdr`), or it is an `unsafe fn` whose conditions are under
`# Safety`. A `SAFETY:` note in a safe function states why the premise
holds there; it never hands the premise to the caller. Functions that
only compare or store such a pointer (the gray list, the records'
lookups by address) may take it raw.

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

Three CI checks cover `unsafe`:

- the `unsafe-drift` job in `.github/workflows/ci.yml` counts the sites
  in every `.rs` file under `crates/` on every push, `luna-jit-llvm`
  included although CI does not build it, and fails above the ceiling.
  The ceiling is the exact count, **1907**, with no headroom;
- the same job runs `scripts/unsafe-accounting.py`, which puts every file
  in one row of the table in §2 and fails when a row, the table's total,
  the snapshot in §1 or the ceiling differs from the count in the tree,
  or when a file fits no row: rows that are off by the same amount in
  opposite directions leave the total right and are caught here;
- `clippy::undocumented_unsafe_blocks` is set in the workspace
  `[lints.clippy]` table, and the lint job runs clippy with
  `-D warnings`, so a block or `unsafe impl` without a `SAFETY:` note
  fails the build.

A change that adds `unsafe`:

1. puts a `SAFETY:` note on the block that states its premise at that
   site, and a `# Safety` section on any `unsafe fn`;
2. raises the ceiling in `ci.yml` to the new exact count and says why
   in the commit message, or removes an equivalent site elsewhere;
3. updates the rows of §2 and the snapshot of §1 it changes (a new
   module that fits no row gets one, and a rule in the script).

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
border, added 4. In `luna-core` 1: `Heap::try_new_table_presized`, which
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
the other arms do, and added 3. That is 1421.

On musl the main thread's stack bounds are read as on glibc, its top less
the stack size limit (musl's `pthread_getattr_np` gives only the part the
kernel has mapped so far): one foreign block and one call, 2 more. That is
1423.

Code the LLVM backend's compile thread writes is made runnable on the
Vm's thread by an `isb` the Vm's thread runs as it takes the entry (one
block in luna-core's `jit`, for every operating system), in place of the
two `membarrier` calls in `jit_backend` that covered aarch64 Linux only:
1 less. That is 1422.

A small block from the system allocator is resized by moving it to a
fresh block (`runtime/mem/resize.rs`: the function, the block that
allocates, copies and frees, and the call in `MemCtx::realloc`), since
glibc's `realloc` takes the arena lock once the process has a second
thread and its per-thread cache of small blocks does not: 3 more. That
is 1425.

A system-allocator context keeps freed blocks of 1 KiB to 256 KiB for its
next allocations of the same layout (`runtime/mem/pool.rs`, so a process
with a second thread skips glibc's arena lock for them too): giving the
kept blocks back when a cycle trims the pool and when the context goes,
taking one, moving a block through the pool on a resize (the function and
its two blocks), zeroing a kept block for `alloc_zeroed` and taking a
fresh zeroed one when none is kept, and freeing a block the pool has no
room for: 10 more in `runtime/mem`. A baseline trace
keeps the record it was lowered from, to lower it again for the optimizing
tier, and the record's function handles need `Send` and `Sync` to live in
the trace's tier-up state (`jit_backend/trace/tier_source.rs`): 2 more.
That is 1437.

The slow paths of the interpreter loop's comparisons moved into functions
of their own that read the operands through the register pointers the
loop already holds (`vm/exec/compare.rs`: three `unsafe fn` and their
three reads), so the loop's frame keeps no slots for them: 6 more. That
is 1443.

The fast loop prepares numeric `for` loops itself (two blocks, one per
register layout, reading and writing the control registers in place),
and its `for` steps read a tag a second time so that the first read can
be compared in memory (`raw_tag_again`) and store a float's payload
without its tag (`put_flt`), each an `unsafe fn` with its read or
write: 6 more. That is 1449.

A numeric `for` writes its control values with the bytes between tag and
payload zeroed, and on aarch64 its step reads each value's tag and
payload as two words in one load and tests the tag as a whole word
(`vm/exec/num_words.rs`: the write, the two reads, which are `asm!`
because Rust code may not read padding, each an `unsafe fn` with its
block, and the slow path's rewrite of the values; and one block in each
of the two aarch64 step arms): 9 more. That is 1458.

A numeric `for` step tests a whole first word where it does not test a
control value's tag (`num_words.rs`: a read of one word, an `unsafe fn`
with its block for each of x86-64 and aarch64; the write that unseals a value after a debug
write; the empty `asm!` that orders the aarch64 back-edge target before
the loads), and a test drives `lua_setlocal` from Rust (its `extern`
block, the C function's block and the test's): 9 more. That is 1467.

Every allocation can now fail with the memory error: the emergency
collection and the constructors that can fail, the C API functions that
raise the memory error, and the table writes of the libraries moved into
helpers that need no `unsafe` of their own, and the marker tracing gray
objects where a walk of the object list finds them when its stack cannot
grow. That is 6 more in `vm/exec`, 7 in the heap, 1 each in the table and
in `vm/exec/index_*`, 3 elsewhere in `runtime`, 4 in the C API and 2 in
unit tests; 14 fewer in the libraries and 2 fewer elsewhere in `vm`: 8
more in all. That is 1475.

A native call pushes its activation record without checking the
capacity, after the call has made room for it (`LVec::push_in_room`, an
`unsafe fn` with its block, and the call's block): 3 more. That is 1478.

`SendVm::get_global` reads the globals table through the safe handle
instead of a raw pointer: 1 fewer. That is 1477.

A coroutine's status and the error it died with are written through two
methods of the VM (`set_coro_status`, `set_coro_error`), each holding the
one block, instead of a block at each place a resume writes them: 2
fewer. That is 1475.

A finalizer whose run was cut short (an error, or too few C levels left)
goes back on the queue marked as finalized, so it cannot be registered
again while it waits (the flag write in `Heap::requeue_tobefnz`): 1 more.
That is 1476.

`collectgarbage("count")` counts PUC's object sizes, and an object luna
makes that PUC does not (its own libraries' objects, the environment cell
of a 5.1 closure) is marked so that its free is not counted either: the
two accessors of that header flag hold a block each, and a text load
keeps the tables PUC's parser fills, written through one block: 3 more in
the heap and `vm/exec`. That is 1479.

Two `luna_jit_*` helpers were added in `luna-jit-helpers/src/guards.rs`,
each a `pub unsafe extern "C" fn` whose body takes the current Vm (one
block), as every helper does: `luna_jit_due_words`, which tells compiled
code where the allocation count and the point a collection step falls
due at lie, so that a check compares them inline and calls into the
runtime only once the count has reached that point; and
`luna_jit_str_sub_checked`, the `string.sub` of 5.1 and 5.2 traces,
which check for a collection before the string is made, so that neither
helper reads the dialect as it runs. 4 more in `luna-jit-helpers`; the
tree's count stays at the ceiling, 1802, other sites having gone since.

`LVec::extend_from_front` (`runtime/mem/vec/clone.rs`) appends a copy of
the vector's first elements into the room it reserved, one block, so that
`string.rep` doubles the copies it made without filling the room with
zeros first. 1803.

A trace's concat passes its operands to one helper,
`luna_jit_op_concat_regs`, which stores them and runs the concat
(`Vm::jit_concat_regs` packs each operand and reads the results back, 2
sites), in place of a `luna_jit_spill_to_stack` call per operand and
`luna_jit_stack_load` calls after. `luna_jit_stack_update_raw` and
`luna_jit_stack_load` are gone (6 fewer in `luna-jit-helpers`, 1 fewer in
`vm/exec`). The root lists the helpers take are linked in place instead
of copied, and the collector reads them where they are (1 in
`gc/roots.rs`). 1799.

A collection step that falls due in a trace runs where the trace is:
`luna_jit_gc_due` takes the trace's register buffer as its exit there
leaves it and reads the frame's registers from it to write them back
before the step (1 in `luna-jit-helpers`). The trace's entry and exit
copies read and write each register through one function each, so they
stay at one site apiece. 1800.

A native's results go to the stack as a tag byte and a payload word
(`Value::copy_raw`) through a safe function taking two references: a
result just made sits in memory as those two stores, and a 16-byte copy
of it waits for them to reach the cache on every call (1 in
`runtime/value/raw/encode.rs`). 1801.

The work of marking a leaf is read through a handle: the model's
`leaf_work` is a safe function taking the object's `Gc` (one block for
the cast its tag names), called from one out-of-line method of the
marker (1 fewer in `runtime/puc`). 1800.

No safe function dereferences a raw object pointer it is given. The
check that an object a load set aside is still off the lists reads the
object's link, so it is written in the two methods that walk the heap's
own list of those objects rather than in a safe helper taking any
pointer (1 more in `runtime/puc`); the barrier between two of 5.1's
environment records dereferences the records' own pointers, which the
arguments are only compared with. 1801.

The stores into a table that an embedder or luna's other crates make go
through the table's handle, which borrows the table in one block
(`table/store.rs`, 1 more) and takes the write barrier itself, so the
blocks that borrowed a table for those stores are gone: 44 fewer in
`vm/exec`, `vm` and its libraries, the unit and integration tests,
`lua_facade` and the JIT's helpers. The two stores
that leave the read-only test and the barrier to the JIT's helpers are
`unsafe fn`s in `__private` (4 more). 1762.

Those stores, and `Heap`'s constructors and write barriers, left the public
API: a store or barrier given another heap than the one that made the
object could free an object the VM still uses, from safe code. luna's other
crates reach the stores and barriers as the `unsafe fn`s of
`__private::{TableStores, HeapBarriers}`, whose conditions name the heap
(14 in `__private`). The C API's barriers after it writes a thread's stack
or a C function's upvalue (15), the JIT's helpers' stores and barrier (9)
and a JIT test's metatable store (1) are blocks stating that the object is
the running Vm's; a test of a read-only table's stores (1 in
`table/store.rs`). 1802.

`Heap::new_on` left the public API too, and `__private::new_heap` became
an `unsafe fn`: a heap of one's own can be dropped while handles to its
objects live on (1 in `__private`, 16 blocks in the tests, luna-aot and
luna-fuzz that make one). 1819.

A JIT test sets its metatable through `Vm::set_metatable` (1 fewer). 1818.

`__private::VmHeap::heap_mut` became an `unsafe fn`: safe code could swap
two VMs' heaps through it and free objects a VM still uses (3 in
`__private`, with the block of the doc example that shows it). The JIT's
helpers that allocate a table (4 in `luna-jit-helpers`) and the AOT key
resolver's intern (1 in `luna-runtime-helpers`) call it in a block; the
calls already in a block, the C API's barriers and the JIT's stores, state
that the heap stays in place. The tests that only allocated or read
through it use `Vm`'s public methods or `heap()`. 1826.

A backend vouches for the code it hands the interpreter: `IntChunkCompiler`
and `TraceCompiler` are `unsafe trait`s (2 in `jit`), implemented with
`unsafe impl` by the null backend (2 in `jit`), luna-jit's Cranelift and
LLVM backends (4), luna-jit-llvm (2), luna-aot's harvester (1) and a test
(1). A compiled trace is made from its parts by the `unsafe fn`s
`CompiledTrace::from_parts` and `from_aot_meta` (3 in `jit`, with the block
in `from_aot_meta`), whose calls in luna-jit's compile and adopt paths (2),
the AOT resolver (1) and tests (2) are blocks. `Proto`'s JIT fields are
crate-internal; luna's crates set a pending method entry through
`ProtoTraceInternals::set_jit_next` and install an AOT trace through
`__private::install_aot_trace`, both `unsafe fn`s (3 in `__private`), from
the LLVM method JIT (2) and tests (4). The `jit` module and `CompiledTrace`
show what no longer compiles (3 in `jit`). Two comments that quoted the
pattern went with the code they described (1 fewer in `vm/exec`, 1 in
`jit`). 1856.

Vms given clones of one `MemOwner` no longer share an allocation context,
whose memory error string belonged to whichever heap made it last: each
heap takes a context of its own, and the host function or policy and the
byte count move to a budget the contexts share. A context reads its budget
through the pointer it holds, and the last context using a budget frees it
(2 in `runtime/mem`). 1858.

The JIT state and a trace's tier-up source left safe reach: a `&mut
JitState` let a VM's code storage be swapped for another's, and a tier
source could be moved to another trace. `__private::VmJitState::jit_mut`
is an `unsafe fn` (3 in `__private`, with a doc example's block), the
helpers' few writes it still serves are blocks (3 in `luna-jit-helpers`),
and the rest go through narrow safe methods; `__private::put_tier_source`
is an `unsafe fn` (1 in `__private`), called by the LLVM tier (2 in
`jit_backend`); a compiled trace's doc examples show both (2 in `jit`).
`jit_concat_regs` and `jit_gc_step`, which take raw register words, are
`unsafe fn`s (8 in `__private`, with the forwarding blocks and doc
examples; 2 in `vm/exec` for the crate functions). Containers are made on
a context only inside luna: `__private::{lvec_new, lvec_with_capacity,
lbox_new, lany_new}` (4 in `__private`) and `jit_str_buf_acquire` (2) are
`unsafe fn`s, the C API's calls are blocks (3 in `capi`). 1888.

## 5. Public `unsafe` surface

### `pub unsafe fn` (1 in the public API, 34 in `luna_core::__private`)

| Location | Function | Why |
|---|---|---|
| `runtime/mem/ctx.rs` | `MemOwner::raw` | A host that supplies a `lua_Alloc`-style function vouches for it; the safe way to watch or limit a Vm's memory is `MemOwner::policy`. |
| `__private::GcInternals` | `from_ptr` | Rebuilds a handle from a pointer compiled code passes around; the pointer must be a live object. |
| `__private::GcInternals` | `as_mut` | Mutation through a handle by luna's own crates; embedders use `TableBuilder` / `LuaUserdata`. |
| `__private::ValueInternals` | `pack` | Low-level constructor from the array-part encoding, used by the JIT and the C API. |
| `__private` | `new_heap` | A heap of its own, for luna's tools and tests: no handle to its objects is used after it is dropped, and its objects go into no other heap's. |
| `__private::TableStores` | `store`, `store_int`, `store_existing`, `store_metatable` | A store into a table, with the write barrier: the heap passed is the one that made the table, the key and value are its objects, and no other reference to the table is held across the call. |
| `__private::CompiledTrace` | `from_parts`, `from_aot_meta` | The trace the dispatcher enters, made from a backend's parts or an AOT binary's metadata: the entry is code compiled against the tags and window the parts state, alive while any Vm holds the trace; the cells whose addresses the code holds are the parts' own; a tier-up source was made for this trace in the JIT storage of the Vm it is installed on. |
| `__private` | `install_aot_trace` | An AOT binary's trace on a prototype: as `from_parts`, and compiled for that prototype at its head pc. |
| `__private` | `set_tier_up_loop` | The optimizing tier's loop function written into a trace's loop cell (and the tier-up marked tried): the function was compiled for this trace's loop entry and shares its exits and cells. |
| `__private` | `put_tier_source` | The source an installed trace is compiled from at the optimizing tier: made for this trace by its backend, in this VM's JIT storage. |
| `__private::VmJitState` | `jit_mut` | The JIT state, to change: not moved or replaced, and the fields the dispatcher and the collector trust written only as the interpreter writes them. |
| `__private::VmJitFrames` | `jit_concat_regs` | A concat's operands given as tag and payload words: each is the register's own. |
| `__private::VmJitAlloc` | `jit_gc_step`, `jit_str_buf_acquire` | A step run in a trace's frame from the register words it gives; a string buffer on the VM's context, given back before the VM goes. |
| `__private` | `lvec_new`, `lvec_with_capacity`, `lbox_new`, `lany_new` | A container on a context, for luna's other crates: an owner of the context outlives it. |
| `__private::ProtoTraceInternals` | `set_jit_next` | The method entry a background compile hands over: the cell holds 0, the none marker, or code with the prototype's method-JIT ABI. |
| `__private::VmHeap` | `heap_mut` | The VM's heap, to allocate in and write to: it is not moved or replaced through the reference, and the objects written through it are the VM's. |
| `__private::HeapBarriers` | `barrier_forward`, `barrier_back`, `barrier_store` | The write barriers, for the C API and the JIT's helpers: the objects are this heap's. |
| `__private::TableInternals` | `set_unguarded`, `set_int_unguarded` | The JIT's store helpers write a table without the read-only test and the write barrier, which they make themselves around the store. |
| `__private::MemCtxInternals` | `set_raw_alloc` | `lua_setallocf`: the C API swaps the host function, which must accept the previous one's blocks. |
| `__private::VmJitFrames` | `jit_set_top`, `jit_push_inlined_frame`, `jit_handoff_frame` | Compiled code's helpers set the stack top, rebuild an inlined function's frame, and hand a level to the interpreter at a frame and instruction compiled code vouches for. |
| `__private::VmCApiLoad` | `host_set_first_upvalue` | `lua_load` sets the first upvalue of the function a load just made; on any other function it would change an upvalue other functions or a stack slot share. |
| `__private::VmJitAlloc` | `jit_closure` | A closure capturing registers of the frame compiled code says is running. |

`luna_core::__private` is not part of the public API: luna's own crates
use it, nothing in it is covered by semantic versioning, and an embedder
does not need it. The public API needs no `unsafe` but `MemOwner::raw`.

### `pub unsafe extern "C" fn` (208)

| Location | Count | Why |
|---|---:|---|
| `luna-jit/src/capi*` | 143 | the C API, called from C with raw `lua_State` pointers (the functions written in C are reached through naked jumps, which are not `unsafe fn`s) |
| `luna-jit-helpers/src/*` | 59 | the `luna_jit_*` helpers compiled code calls (§3.5); each has a `# Safety` section |
| `luna-runtime-helpers/src/*`, `luna-aot` | 6 | `luna_aot_run_dialect`, the AOT binary's entry, called by the generated C `main` with the dialect the script was compiled for; `luna_aot_run`, the same entry for 5.5 |

### `unsafe impl Send` / `Sync` (13)

| Location | Impl | Why |
|---|---|---|
| `runtime/function.rs` | `LuaClosure: Send + Sync` | The closure type holds no `Rc` / `RefCell`; `Gc<LuaClosure>` stays `!Send`. |
| `runtime/table.rs` | `Table: Send + Sync` | Same shape. |
| `vm/send_vm.rs` | `SendVm: Send` | `feature = "send"`: every method takes the `RwLock` write guard before forming `&mut Vm`, so the `Vm` keeps one mutator at a time; `SendVm` is deliberately not `Sync` (see [`threading.md`](threading.md)). |
| `jit_backend/jit_handle.rs` | `JitHandle: Send` | Owns its `JITModule` (through `SendJitModule`) by value together with the entry pointer into that module's code, so the two move between threads together. |
| `jit_backend/trace/compile.rs` | `TraceHandle: Send` | Required by the thread-local trace cache; the cache is only touched from its own thread. |
| `jit_backend/trace/lir/code.rs` | `CodeArena: Send` | Owns the baseline tier's code pages of one `Vm`; the code in them only runs on the thread that owns that `Vm`. |
| `jit_backend/trace/lir/code.rs` | `Spare: Send` | A freed chunk of code pages, writable again, that nothing points into; the pool hands each to one arena at a time. |
| `jit_backend/trace/tier_source.rs` | `TierRecord: Send + Sync` | The record a baseline trace keeps to be lowered again for the optimizing tier: its function handles are only read, on the thread of the Vm that owns the trace, and only the lowered instructions go to the LLVM compile thread. |
| `luna-jit-llvm` `storage.rs` | `EnginePair: Send` | An LLVM engine and its context, built on one thread and handed whole to the Vm that runs the code; nothing else holds either once the engine is finalized. |
| `luna-runtime-helpers` `jit_helpers_pin.rs` | `PinnedFn: Sync` | The elements of an immutable static of helper addresses, never written through or dereferenced. |

Test and bench code adds three more `unsafe impl`s that are not part of
any library: two `GlobalAlloc` wrappers over `System` (a poisoning one
in `luna-core/tests/gc_stress_poison.rs`, a counting one in
`luna-jit/tests/jit_code_freed_with_vm/main.rs`) and `Send` for the no-op
wrapper in `luna-jit/benches/bench_send_overhead.rs`.

## 6. Reproducing the counts

```sh
grep -rE --include='*.rs' 'unsafe (\{|fn |impl |trait |extern )' crates | wc -l
# 1907, the ceiling in ci.yml's unsafe-drift job
python3 scripts/unsafe-accounting.py
# every row of §2, the totals and the ceiling against the tree
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
