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
| `unsafe` sites in all crates (every `.rs` file under `crates/`) | **908** | CI ceiling in `.github/workflows/ci.yml::unsafe-drift` |
| of which in tests, benches and examples | 197 | unit-test files under `src/` and the `tests/`, `benches/`, `examples/` trees |
| **`pub unsafe fn` in the public API** | **5** | all `#[doc(hidden)]`, see §5 |
| **`pub unsafe extern "C" fn`** | 75 | the `lua.h` C API (29), the `luna_jit_*` helpers compiled code calls (45, re-exported by `luna-jit`) and the AOT entry (1); see §5 |
| **`unsafe impl Send` / `Sync`** | 8 | see §5 |

A "site" is a line matching `unsafe (\{|fn |impl |trait |extern )`,
the pattern CI counts (§4). Several `unsafe` blocks on one line count
once; a block spread over several lines counts once; a comment that
quotes the pattern counts too.

## 2. Distribution by crate and module

| Crate | Module | Sites | What the `unsafe` is for |
|---|---|---:|---|
| `luna-core` | `vm/exec` fast loop (`fast.rs`, `fast/*`, `fast_arith.rs`) | 56 | reading and writing registers and constants in place through the frame's register window; the running frame pointer; instruction fetch |
| | `vm/exec/index_*` | 38 | table reads and writes the loop finishes itself with the operands read in place; the `__index` / `__newindex` miss paths entered with raw operand pointers |
| | `vm/exec` (other) | 63 | `Gc` handle mutation, frame and stack bookkeeping, coroutine resume, trace entry and exit register copies, the runtime entry points compiled code calls |
| | `runtime/heap*`, `gc_ptr.rs` | 70 | the intrusive mark-sweep heap: allocation, marking, sweeping, finalisation, the `Gc<T>` handle |
| | `runtime/table*` | 48 | the table's raw layout: the node array, the slab-backed array part, tag-driven marking |
| | `runtime` (other) | 36 | string headers and their trailing bytes, the value tag/payload encoding, closure upvalue storage |
| | `vm/lib_*` | 44 | `Gc` handle mutation in the standard library (io handles, `table`, `debug`) and the table writes that build each library |
| | `vm` (other) | 48 | userdata trampolines, typed natives, SendVm, async natives, call-stack walks |
| | `jit`, `frontend` | 10 | trace metadata handed to the backend; interned-name text |
| | unit-test files under `src/` | 13 | tests that inspect raw layouts |
| | `tests/` | 43 | integration tests: a poisoning global allocator, async wakers, userdata internals |
| `luna-jit` | `capi*` | 69 | the `lua.h` C ABI: raw `lua_State` pointers and C strings across the boundary |
| | `jit_backend` | 40 | executable code memory, compiled-function entry points, `Send` for handles that own JIT modules |
| | other | 2 | the CLI's `arg` table and the `lua_facade` table handle |
| | unit-test files under `src/` | 45 | tests that call compiled code or the `extern "C"` helpers directly |
| | `tests/`, `benches/`, `examples/` | 58 | the C API from Rust, a counting global allocator, the `send` overhead bench |
| `luna-jit-helpers` | | 139 | the `luna_jit_*` `extern "C"` helpers compiled code calls (§3.5) |
| `luna-jit-llvm` | `src/` | 8 | LLVM execution engines and the register-file GEPs |
| | `tests/` | 35 | calling LLVM-compiled chunks |
| `luna-runtime-helpers` | | 38 | the AOT binary's C entry, the linker-section walkers (§3.6), the PE header walk on Windows, the helper link anchor |
| `luna-aot` | | 3 | the embedded bytecode section of an AOT binary |
| `llvm-jit-probe` | | 2 | the LLVM toolchain probe |
| `luna-jit-derive`, `luna-tools`, `luna-fuzz` | | 0 | |
| **Total** | | **908** | |

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
the bound, safe indexing is used instead (the frame below a returning
one, the frame pop).

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

### 3.6 AOT binaries

An AOT binary finds its embedded bytecode, trace metadata, string keys
and inline frame chains in linker sections, bracketed by
`__start_` / `__stop_` symbols (ELF), `section$start` / `section$end`
(Mach-O), or located by walking the PE headers (Windows). The walkers
are `unsafe fn`s whose contract is the section layout `luna-aot` emits;
they turn the section into a slice of index entries once and keep the
`unsafe` to the reads of each entry's payload and the write of its
slot. The PE walk computes addresses with wrapping arithmetic and reads
only header bytes inside the first pages the loader maps.

## 4. CI enforcement

Two CI checks cover `unsafe`:

- the `unsafe-drift` job in `.github/workflows/ci.yml` counts the sites
  in every `.rs` file under `crates/` on every push, `luna-jit-llvm`
  included although CI does not build it, and fails above the ceiling.
  The ceiling is the exact count, **908**, with no headroom;
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
908, the ceiling now.

## 5. Public `unsafe` surface

### `pub unsafe fn` (5, all `#[doc(hidden)]`)

| Location | Function | Why |
|---|---|---|
| `runtime/gc_ptr.rs` | `Gc::<T>::from_ptr` | Rebuilds a handle from a pointer compiled code passes around; the pointer must be a live object. |
| `runtime/gc_ptr.rs` | `Gc::<T>::as_mut` | Internal mutation; embedders use `TableBuilder` / `LuaUserdata`. |
| `runtime/value.rs` | `Value::as_closure_unchecked` | JIT hot path; skips the tag match. Safe alternative: match `Value::Closure(_)`. |
| `runtime/value.rs` | `Value::as_int_unchecked` | Same shape. |
| `runtime/value_raw.rs` | `Value::pack` | Low-level constructor from the array-part encoding, used by the JIT and the C API. |

None of these appears in the `cargo doc` view of the API.

### `pub unsafe extern "C" fn` (75)

| Location | Count | Why |
|---|---:|---|
| `luna-jit/src/capi*` | 29 | the `lua.h` C API, called from C with raw `lua_State` pointers |
| `luna-jit-helpers/src/*` | 45 | the `luna_jit_*` helpers compiled code calls (§3.5); each has a `# Safety` section |
| `luna-runtime-helpers/src/lib.rs` | 1 | `luna_aot_run`, the AOT binary's entry, called by the generated C `main` |

### `unsafe impl Send` / `Sync` (8)

| Location | Impl | Why |
|---|---|---|
| `runtime/function.rs` | `LuaClosure: Send + Sync` | The closure type holds no `Rc` / `RefCell`; `Gc<LuaClosure>` stays `!Send`. |
| `runtime/table.rs` | `Table: Send + Sync` | Same shape. |
| `vm/send_vm.rs` | `SendVm: Send` | `feature = "send"`: every method takes the `RwLock` write guard before forming `&mut Vm`, so the `Vm` keeps one mutator at a time; `SendVm` is deliberately not `Sync` (see [`threading.md`](threading.md)). |
| `jit_backend/jit_handle.rs` | `JitHandle: Send` | Owns its `JITModule` (through `SendJitModule`) by value together with the entry pointer into that module's code, so the two move between threads together. |
| `jit_backend/trace/compile.rs` | `TraceHandle: Send` | Required by the thread-local trace cache; the cache is only touched from its own thread. |
| `luna-runtime-helpers` `jit_helpers_pin.rs` | `PinnedFn: Sync` | The elements of an immutable static of helper addresses, never written through or dereferenced. |

Test and bench code adds three more `unsafe impl`s that are not part of
any library: two `GlobalAlloc` wrappers over `System` (a poisoning one
in `luna-core/tests/gc_stress_poison.rs`, a counting one in
`luna-jit/tests/jit_code_freed_with_vm.rs`) and `Send` for the no-op
wrapper in `luna-jit/benches/bench_send_overhead.rs`.

## 6. Reproducing the counts

```sh
grep -rE --include='*.rs' 'unsafe (\{|fn |impl |trait |extern )' crates | wc -l
# 908, the ceiling in ci.yml's unsafe-drift job
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
