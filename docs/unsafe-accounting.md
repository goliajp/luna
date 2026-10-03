# Unsafe accounting

luna uses `unsafe` Rust in a small number of bounded categories: the GC
heap's `NonNull`-based pointer model, the interpreter's in-place access
to registers, constants and table storage, the JIT backend's FFI to
Cranelift-emitted machine code, the `lua.h`-compatible C ABI, the
AOT-binary runtime entry, and the cross-thread `feature = "send"`
SendVm newtype. Every `unsafe` block outside unit-test code carries a
`SAFETY:` note that states the premise it relies on
(`clippy::undocumented_unsafe_blocks` finds none without one there; 49
blocks in the test files under `src/` have no note yet).

This page is the human-readable companion to `cargo-geiger`'s
machine-grepable summary (see §6). For the **embedder-surface
contract** (no `unsafe` required to use `luna-core` or `luna-jit`'s
public API) see [`security.md`](security.md) §5.

---

## 1. Snapshot (4.1 development, October 2026)

| Metric | Count | Notes |
|---|---:|---|
| `unsafe` sites in the CI-watched scope (`luna-core` + `luna-jit` `src/`) | **575** | CI ceiling in `.github/workflows/ci.yml::unsafe-drift` |
| of which in test code under `src/` | 57 | unit-test files beside the code they test |
| Sites in `luna-jit-helpers` | 136 | `extern "C"` helpers the JIT and AOT code call; outside the ceiling |
| Sites in `luna-jit-llvm` | 115 | LLVM backend FFI; outside the ceiling |
| Sites in `luna-runtime-helpers` | 31 | AOT-binary runtime entry, linker section walkers; outside the ceiling |
| Sites in `luna-aot` | 4 | trace code handoff; outside the ceiling |
| Sites in `luna-jit-derive`, `luna-tools` | 0 | |
| **`pub unsafe fn` in the public API** | **4** | all `#[doc(hidden)]`, see §5 |
| **`unsafe impl Send` / `Sync`** | **7** | see §5 |

A "site" is a line matching `unsafe (\{|fn |impl |trait |extern )`,
the pattern CI counts (§4). Several `unsafe` blocks on one line count
once; a block spread over several lines counts once.

## 2. Distribution by module

| Module | Sites | What the `unsafe` is for |
|---|---:|---|
| `luna-core` `vm/exec` fast loop (`fast.rs`, `fast/*`, `fast_arith.rs`) | 56 | reading and writing registers and constants in place through the frame's register window; the running frame pointer; instruction fetch |
| `luna-core` `vm/exec/index_*` | 40 | table reads and writes the loop finishes itself with the operands read in place; the `__index` / `__newindex` miss paths entered with raw operand pointers |
| `luna-core` `vm/exec` (other) | 69 | `Gc` handle mutation, frame and stack bookkeeping, coroutine resume, trace entry and exit register copies |
| `luna-core` `runtime/heap*`, `gc_ptr.rs` | 50 | the intrusive mark-sweep heap: allocation, marking, sweeping, finalisation, the `Gc<T>` handle |
| `luna-core` `runtime/table*` | 48 | the table's raw layout: the node array, the slab-backed array part, tag-driven marking |
| `luna-core` `runtime` (other) | 35 | string headers and their trailing bytes, the value tag/payload encoding, closure upvalue storage |
| `luna-core` `vm/lib_*` | 46 | `Gc` handle mutation in the standard library (io handles, `table`, `debug`) |
| `luna-core` `vm` (other) | 49 | userdata trampolines, typed natives, SendVm, async natives, call-stack walks |
| `luna-core` `jit`, `frontend` | 11 | trace metadata handed to the backend; interned-name text |
| `luna-jit` `capi*` | 69 | the `lua.h` C ABI: raw `lua_State` pointers and C strings across the boundary |
| `luna-jit` `jit_backend` | 42 | executable code memory, compiled-function entry points, `Send` for handles that own JIT modules |
| `luna-jit` (other) | 3 | the CLI and the `lua_facade` handle types |
| test code under `src/` (both crates) | 57 | unit tests that call the `extern "C"` helpers or inspect raw layouts |
| **Total** | **575** | |

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

When Cranelift-compiled code calls back into Rust through the
`luna_jit_*` `extern "C"` helpers (in `luna-jit-helpers`), the helpers
reach the active Vm through a thread-local that a `JitVmGuard` holds
for the duration of a JIT slice; their `SAFETY` notes cite the guard.
The C API (`luna-jit/src/capi*`) receives raw `lua_State` pointers and
C strings across the ABI boundary. `Box::into_raw` /
`Box::from_raw` pairs move trace metadata between Cranelift's symbol
table and the trace cache; each `into_raw` has one matching
`from_raw` on eviction.

## 4. CI enforcement

The `unsafe-drift` job in `.github/workflows/ci.yml` counts the sites
in `crates/luna-core/src` and `crates/luna-jit/src` on every push and
fails above the ceiling. The ceiling is the exact count, **575**, with
no headroom. A change that adds `unsafe`:

1. puts a `SAFETY:` note on the block that states its premise, and a
   `# Safety` section on any `unsafe fn`;
2. raises the ceiling in `ci.yml` to the new exact count and says why
   in the commit message, or removes an equivalent site elsewhere.

The ceiling is never raised to a round number to make room.

History: 461 sites at v1.1, 490 at v1.3 (ceiling 490), 470 at the
start of October 2026, after 4.0.0.
The October 2026 interpreter performance work raised the count to 613
(mostly §3.2 and §3.3, plus unit tests moved out of `jit_backend` into
files of their own, which moved sites without adding any). An audit of
every site added in that work then replaced the ones a safe form covers
at no measured cost (pointer arithmetic with `wrapping_add`, indexing
whose bound the optimiser sees, `Vec::truncate`), folded blocks that
share a premise into one, made the table miss paths that take raw
pointers `unsafe fn`, kept `Value::pack_into` crate-private, and set
the ceiling to the resulting 575.

The scope leaves out `luna-jit-helpers`, `luna-jit-llvm`,
`luna-runtime-helpers` and `luna-aot` (their counts are in §1): their
`unsafe` is FFI to generated code or to the linker, with a different
invariant shape from the runtime's.

## 5. Public `unsafe` surface

### `pub unsafe fn` (4, all `#[doc(hidden)]`)

| Location | Function | Why |
|---|---|---|
| `runtime/gc_ptr.rs` | `Gc::<T>::as_mut` | Internal mutation; embedders use `TableBuilder` / `LuaUserdata`. |
| `runtime/value.rs` | `Value::as_closure_unchecked` | JIT hot path; skips the tag match. Safe alternative: match `Value::Closure(_)`. |
| `runtime/value.rs` | `Value::as_int_unchecked` | Same shape. |
| `runtime/value_raw.rs` | `Value::pack` | Low-level constructor from the array-part encoding, used by the JIT and the C API. |

None of these appears in the `cargo doc` view of the API.

### `unsafe impl Send` / `Sync` (7)

| Location | Impl | Why |
|---|---|---|
| `runtime/function.rs` | `LuaClosure: Send + Sync` | The closure type holds no `Rc` / `RefCell`; `Gc<LuaClosure>` stays `!Send`. |
| `runtime/table.rs` | `Table: Send + Sync` | Same shape. |
| `vm/send_vm.rs` | `SendVm: Send` | `feature = "send"`: every method takes the `RwLock` write guard before forming `&mut Vm`, so the `Vm` keeps one mutator at a time; `SendVm` is deliberately not `Sync` (see [`threading.md`](threading.md)). |
| `jit_backend/jit_handle.rs` | `JitHandle: Send` | Owns its `JITModule` (through `SendJitModule`) by value together with the entry pointer into that module's code, so the two move between threads together. |
| `jit_backend/trace/compile.rs` | `TraceHandle: Send` | Required by the thread-local trace cache; the cache is only touched from its own thread. |

## 6. Reproducing the counts

```sh
grep -rE 'unsafe (\{|fn |impl |trait |extern )' \
    crates/luna-core/src crates/luna-jit/src | wc -l
# 575, the ceiling in ci.yml's unsafe-drift job
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
