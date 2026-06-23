# Unsafe accounting

luna uses `unsafe` Rust in three categories: the GC heap's
`NonNull`-based pointer model, the JIT backend's FFI to
Cranelift-emitted machine code, and the `lua.h`-compatible C ABI.
Every `unsafe` block carries a `SAFETY:` justification.

This page is the human-readable companion to `cargo-geiger`'s
machine-grepable summary. Run `cargo geiger -p luna --bin luna` for
the tool's per-crate breakdown; the numbers and pattern explanations
below give the equivalent picture without requiring the install.

## v1.1 snapshot

| Metric | Count |
|---|---|
| Total `unsafe` sites (blocks + `fn` decls + `impl` decls) | **461** |
| Sites with `// SAFETY:` annotations | **394** |
| Coverage | **85%+** (the remaining ~67 are short single-line `unsafe { ... }` blocks inside helper functions whose enclosing fn carries the SAFETY rationale at the `unsafe fn` boundary) |
| `pub unsafe fn` in public API | **4** (all `#[doc(hidden)]` per A4) |
| `unsafe impl Send/Sync` | **5** (load-bearing — see [`architecture.md`](architecture.md) §"Threading model") |

A6 (Track A item) brought SAFETY comment coverage to 100% across
all `unsafe { ... }` blocks that lacked one; the residual count
above reflects blocks where the SAFETY rationale lives at the
enclosing `unsafe fn` declaration (still annotated, just not at
the block level).

## Distribution by crate

```
crates/luna-core/src/
  runtime/heap.rs           ~110  GC mark-sweep, NonNull deref, intrusive list walks
  runtime/value.rs           ~25  Value::pack / Gc<T> field access (doc-hidden)
  runtime/userdata.rs        ~15  Userdata payload reads (file handle conversion)
  vm/exec.rs                ~100  Dispatcher hot loop, Gc<T> derefs, CoroSavedCtx swap
  vm/builtins.rs             ~15  intern + table set helpers
  vm/lib_os_io.rs            ~10  std::fs::File + std::process::Command FFI
  vm/lib_strpack.rs           ~5  string.pack reads
  vm/typed_native.rs          ~5  fn-pointer transmute (typed wrapper trampolines)

crates/luna/src/
  jit_backend/mod.rs        ~75  Cranelift FFI, JIT_VM thread-local, luna_jit_*
                                 #[unsafe(no_mangle)] extern "C" helpers
  jit_backend/trace.rs      ~80  Trace IR lowering scratch, register class transitions
  capi.rs                   ~30  lua.h-compatible *mut lua_State entry points
```

## Pattern catalog (why these unsafe blocks exist)

### 1. `Gc<T>` deref (~250 of 461)

luna's GC handles are `Gc<T> = NonNull<T>` over an intrusive
mark-sweep heap. Every read or write through a handle is a
`unsafe { *gc.as_ptr() }` or `unsafe { &mut *gc.as_ptr() }`.

The safety contract is documented in `runtime/heap.rs:5-7`: *"the
runtime is single-threaded; a `Gc` pointer is valid until a
`collect()` call that does not reach it from the given roots."*
This invariant is enforced by:

- `Vm: !Send + !Sync` (A7)
- The Vm's `gc_roots()` aggregator covering every reachable
  handle (host_roots, globals, stack, frames, metatables, hook
  function, current coroutine, etc.)
- `Vm: !Send` prevents cross-thread access; the single-threaded
  contract holds by construction.

### 2. JIT FFI thread-locals (~80 of 461)

When Cranelift-compiled code calls back into Rust via
`luna_jit_*` extern "C" helpers, those helpers need to reach the
active Vm. luna stores `&mut Vm` in a thread-local (`JIT_VM`) at
dispatch entry; the helpers read it back as `&mut *JIT_VM.with(|c| c.get())`.
The `JitVmGuard` RAII type holds the Vm pointer for the duration
of a JIT slice; SAFETY annotations cite the guard's lifetime as
the validity proof.

### 3. `unsafe extern "C" fn` helpers (~30 of 461)

The 26 `luna_jit_*` helpers in `crates/luna/src/jit_backend/mod.rs`
have stable C ABI shapes Cranelift can call. Each is annotated
`#[unsafe(no_mangle)]` (Rust 2024 edition form) so the linker
exposes the symbol; SAFETY rationale at each helper cites the
codegen contract Cranelift establishes (specific register state,
stack layout).

### 4. `Box::into_raw` / `Box::from_raw` pairs (~20 of 461)

Used for transferring ownership of heap-allocated trace metadata
between Cranelift's symbol table and luna's trace cache. Each
`into_raw` is matched by exactly one `from_raw` on the same
allocator path; cleanup runs when the trace is evicted.

### 5. Public `pub unsafe fn` (4 total)

| File:line | Function | Why |
|---|---|---|
| `runtime/heap.rs:130` | `Gc::<T>::as_mut` | `#[doc(hidden)]` per A4. Internal mutation path; embedders use `TableBuilder` (B3) for safe table population. |
| `runtime/value.rs:133` | `Value::as_closure_unchecked` | `#[doc(hidden)]` per A4. JIT hot-path; bypasses tag match. Safe alternative: `Value::Closure(_)` match arm. |
| `runtime/value.rs:156` | `Value::as_int_unchecked` | Same shape as `as_closure_unchecked`. |
| `runtime/value.rs:296` | `Value::pack` | `#[doc(hidden)]` post-A1 Session C. Low-level Value constructor used by JIT codegen + capi. |

None of these appear in `cargo doc`'s public API surface; the
`#[doc(hidden)]` ensures embedders following the rustdoc never
discover them. The compile_fail doctest on `Vm` (A7) doesn't
trigger for these because they're not `Send`/`Sync`-related.

### 6. `unsafe impl Send/Sync` (5 total)

| File:line | Impl | Why |
|---|---|---|
| `runtime/function.rs:419-420` | `LuaClosure: Send + Sync` | `LuaClosure` itself is `Send`-compatible (no `Rc`/`RefCell`); the GC handle `Gc<LuaClosure>` is `!Send` separately. The impl satisfies generic trait bounds elsewhere without changing `Vm: !Send`. |
| `runtime/table.rs:120-121` | `Table: Send + Sync` | Same shape — Table itself is `Send`-compatible, `Gc<Table>` is `!Send`. |
| `jit_backend/trace.rs:1892` | `TraceHandle: Send` | Required by `thread_local!`'s `RefCell<Vec<TraceHandle>>` bound. The TLS context guarantees single-thread access. |

These are NOT load-bearing wrong (don't violate the `Vm: !Send`
contract); they're scaffolding for satisfying trait bounds on
intermediate types. Tracked in `.dev/rfcs/v1.1-audit-pub-surface-safety.md`
for the future Arc-ification sprint (Track A7 forward roadmap).

## Reproducing with cargo-geiger

```sh
cargo install --locked cargo-geiger
cargo geiger -p luna --bin luna
```

The tool's output groups unsafe by category (`unsafe expressions`,
`unsafe traits`, `unsafe functions`, `unsafe impls`) and shows
per-dependency unsafe counts so embedders can audit the supply
chain. luna-core has zero third-party deps so its unsafe footprint
is entirely first-party.

## Ship-time gates

- A6 charter line item: SAFETY: comment coverage to 100% on
  `unsafe { ... }` blocks. **Shipped at commit 7d4a95e** (342
  comments added; 0 TODO(audit) placeholders).
- A4 charter line item: 0 `unsafe` at the embedder surface (public
  `cargo doc` view). **Shipped at commit 37e9414** (the 4 `pub unsafe fn`
  are `#[doc(hidden)]`; `TableBuilder` / `IntoValue` / `native_typed`
  cover the embedder-facing flows).

For the post-v1.1 `feature = "send"` sprint, the `unsafe impl Send/Sync`
pattern category gets re-audited — see
`.dev/rfcs/v1.1-rfc-vm-send-sync.md`.
