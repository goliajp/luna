# Threading

How to use luna in async and multi-threaded host programs.
Snapshot at v1.3.

luna's default `Vm` is **`!Send + !Sync`** — a `Vm` (and any handle
into its GC heap) lives on the OS thread that created it. v1.3 adds
an opt-in `SendVm` newtype (gated behind `feature = "send"`) for
cross-thread embedding; see the
[SendVm section](#feature--send--sendvm-v13) below. This page
covers both shapes plus the canonical async embedding patterns and
the reasoning behind the default `!Send` stance.

---

## Why `Vm: !Send`

luna's GC uses `Gc<T> = NonNull<T>` over an intrusive mark-sweep
heap (not `Rc<RefCell<T>>`, as some Rust-Lua bindings do). The
trace-JIT side-table uses `Rc<CompiledTrace>`. Both are
single-threaded on purpose:

- **Performance**: past benches put the cost of `Gc<T>` → `Arc<RwLock<T>>`
  on the dispatcher hot path at 5-15% on Redis-Lua-shape workloads.
  Single-thread is the perf floor of the v1 line.
- **Simplicity**: the GC mark-sweep walk doesn't need a stop-the-world
  protocol; finalizers fire on the owning thread; trace cache reads
  don't need a lock.
- **Lua's data model**: Lua semantics are themselves single-threaded
  (one Lua thread = one coroutine pool). Multi-thread Lua isn't a
  standard feature, and luna doesn't fake one.

The constraint is **load-bearing** but **invisible** until you try
something that wouldn't work — at which point the Rust type checker
catches it. For example:

```rust
let vm = luna_jit::Vm::new_minimal_with_jit(LuaVersion::Lua55);
tokio::spawn(async move {
    vm.eval("return 1").await  // error[E0277]: `Vm` cannot be sent between threads safely
});
```

luna's `Vm` carries a compile-time `compile_fail` doctest enforcing
this; if a future code change accidentally made `Vm: Send`, the
build fails.

For the v1.3+ `feature = "send"` work, see §[`feature = "send"` — SendVm (v1.3+)](#feature--send--sendvm-v13).

---

## Embedding patterns

### Pattern 1 — Single-thread Tokio runtime

Simplest case. `current_thread` flavor runs the executor on one OS
thread; `Vm` lives there.

```rust
use luna_jit::Vm;
use luna_jit::version::LuaVersion;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut vm = Vm::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    let result = vm.eval_async("return 1 + 2").await?;
    println!("{:?}", result);
    Ok(())
}
```

Use when:
- Your host is a single-tenant async server (one Vm per process)
- You want the simplest possible setup
- Your workload is CPU-light enough that one thread suffices

### Pattern 2 — `LocalSet` on multi-thread Tokio

Use when your host already runs on a multi-thread Tokio runtime
(default `#[tokio::main]`) but you want one `Vm` accessible from
async tasks.

```rust
use luna_jit::Vm;
use luna_jit::version::LuaVersion;
use tokio::task::LocalSet;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local = LocalSet::new();
    local.run_until(async {
        let mut vm = Vm::new_minimal_with_jit(LuaVersion::Lua55);
        vm.open_base();
        vm.eval_async("for i = 1, 1e6 do end").await
    }).await?;
    Ok(())
}
```

`LocalSet::run_until` pins the inner futures (including the `Vm`'s
`EvalFuture<'_>`) to the calling thread. Other Tokio tasks on other
threads continue normally; the Lua-touching async code stays put.

Use when:
- Multi-thread Tokio is already required by other host code
- You want async-await ergonomics around `vm.eval_async`
- You have one Vm (or a small fixed pool, one per `LocalSet`)

### Pattern 3 — `Vm` per OS thread + channels

For host programs that want real parallelism (multiple Vms doing
Lua work concurrently), spawn one OS thread per `Vm` and exchange
data through channels.

```rust
use luna_jit::Vm;
use luna_jit::version::LuaVersion;
use std::sync::mpsc;
use std::thread;

let (req_tx, req_rx) = mpsc::channel::<(String, mpsc::Sender<String>)>();

let worker = thread::spawn(move || {
    let mut vm = Vm::new_minimal_with_jit(LuaVersion::Lua55);
    vm.open_base();
    while let Ok((src, resp_tx)) = req_rx.recv() {
        let r = vm.eval(&src)
                  .map(|v| format!("{:?}", v))
                  .unwrap_or_else(|e| format!("err: {e:?}"));
        let _ = resp_tx.send(r);
    }
});

let (resp_tx, resp_rx) = mpsc::channel();
req_tx.send(("return 1 + 2".into(), resp_tx))?;
println!("{}", resp_rx.recv()?);
drop(req_tx);
worker.join().unwrap();
```

The channel types (`mpsc::Sender<String>`, `String`) are `Send`, so
they cross OS thread boundaries freely. The `Vm` stays on the worker
thread it was created on; only the source code and the result string
move.

For Tokio-flavored hosts, the same idiom uses `tokio::sync::mpsc`
and `tokio::task::spawn_blocking` (or `std::thread::spawn` outside
Tokio's pool) for the worker.

This is the same shape mlua-without-`send` and rlua recommend.

Use when:
- Multiple Lua execution streams need to run truly in parallel
- The work-per-call is significant (channel overhead is amortized)
- Lua state isn't shared across streams (or sharing is mediated by
  the channel protocol)

### Anti-pattern — `tokio::spawn` with a `Vm` capture

```rust
// Compile error — Vm: !Send blocks this at the type level.
let vm = Vm::new_minimal(LuaVersion::Lua55);
tokio::spawn(async move {
    vm.eval_async("...").await  // E0277
});
```

The type system rejects this. The fix is one of Patterns 1-3.

---

## Platform notes

### wasm32

wasm32-unknown-unknown is single-threaded by default (no
`std::thread::spawn` without `wasm-bindgen-rayon` or similar).
`Vm: !Send` doesn't constrain wasm embedders — the platform itself
prevents the question.

`luna-core` builds for `wasm32-unknown-unknown` because it carries
no Cranelift dependency; the `luna` crate (with JIT) does not target
wasm because wasm doesn't allow `mmap` RWX. Use `luna-core` directly
in wasm hosts.

### no_std

Out of scope for v1.1. `luna-core` currently depends on `std`
(`String`, `Vec`, `HashMap`, etc.). A future no_std-capable luna-core
is a longer-term consideration; see `.dev/discussions/0dep-jit-path.md`
for sibling work.

---

## Async embedder API (B10)

`vm.eval_async(src)` and `lua.eval_async(src)` return `!Send` futures
that drive the dispatcher with cooperative yields when the
instruction budget exhausts. Embedders register async natives via
`vm.set_async_native(name, fn_ptr)` (or `lua.set_async_native`),
exposing host-side futures to Lua scripts:

```rust
use luna_jit::Lua;
use luna_core::runtime::Value;
use luna_core::vm::LuaError;
use std::future::Future;
use std::pin::Pin;

fn http_get(
    vm: *mut luna_core::vm::Vm,
    fs: u32,
    _nargs: u32,
) -> Pin<Box<dyn Future<Output = Result<u32, LuaError>>>> {
    Box::pin(async move {
        // SAFETY: see AsyncNativeFn contract.
        let vm = unsafe { &mut *vm };
        let url = match vm.nat_arg(fs, 1, 0) {
            Value::Str(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
            _ => return Err(LuaError(Value::Nil)),
        };
        // let body = reqwest::get(&url).await?.text().await?;
        let body = format!("fake response from {url}");
        let interned = vm.heap.intern(body.as_bytes());
        Ok(vm.nat_return(fs, &[Value::Str(interned)]))
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut lua = Lua::new();
    lua.open_base();
    lua.set_async_native("http_get", http_get)?;
    let body: String = lua.eval_async(r#"
        return http_get("https://example.com")
    "#).await?;
    println!("got: {body}");
    Ok(())
}
```

For a runnable, dependency-free walkthrough, see
[`examples/async_host.rs`](../crates/luna-jit/examples/async_host.rs)
(`cargo run --example async_host -p luna`). It uses a hand-rolled
`block_on` so the example stands alone; production embedders
substitute `#[tokio::main(flavor = "current_thread")]` or a
`LocalSet` for multi-thread Tokio runtimes (see Pattern 2 above).

### Async native limitations (v1.1)

- Calling an async native from sync `vm.eval(...)` errors with a
  typed `LuaError` — embedders must use `eval_async`.
- Cancellation: dropping the future mid-await clears the
  pending-async state but leaves Lua call frames in `vm.frames`.
  Drop the entire Vm when an async eval is cancelled mid-flight.
  See `.dev/rfcs/v1.1-rfc-b10-async-embedder.md` §"Risks".

### Async natives + debug hooks (v1.3 Phase AS)

The v1.1 `[B11]` Rust-side debug hook (`vm.set_rust_debug_hook(...)`)
composes with async natives as of v1.3 Phase AS — embedders see the
same `Call` / `Return` event pair for an async native as for a sync
native or a Lua function. The dispatcher's `Count` and `Line` hook
sites are opcode-driven and have always worked under `async_mode = true`;
the v1.3 fix was the **async-native call boundary itself**:

1. `Call` event fires on the async branch in `vm::exec` **before** the
   future is built (after the result-window pin so a hook body that
   triggers GC observes the correct pin).
2. `Return` event fires from `Vm::commit_async_native_result` after the
   future resolves and results land in the call window.

`Count` hooks carry across slice boundaries — the dispatcher's
`hook.count_left` is a persistent `Vm` field, so a 1000-instruction
count budget survives any number of `Poll::Pending` returns to the
executor. `Line` hooks dedupe across slice boundaries via the same
persistent `hook_lastline` field; a slice ending mid-line and resuming
on the same line will not double-fire.

Worked example — async native bracketed by a Rust hook:

```rust,ignore
use luna_core::vm::Vm;
use luna_core::vm::exec::{HOOK_MASK_CALL, HOOK_MASK_RETURN, RustHookEvent};

fn record(_vm: &mut Vm, evt: RustHookEvent) {
    eprintln!("hook: {evt:?}");
}

vm.set_rust_debug_hook(Some(record), HOOK_MASK_CALL | HOOK_MASK_RETURN, 0);
vm.set_async_native("http_get", http_get)?;
let body = vm.eval_async(r#"return http_get("/")"#).await?;
// stderr shows:  hook: Call  (chunk entry)
//                hook: Call  (http_get entry — fires before .await)
//                hook: Return (http_get exit — fires after .await resolves)
//                hook: Return (chunk exit)
```

Hook events fire only on **completed** semantic boundaries; the
cooperative yield mid-native (the `.await`) does not fire any hook
event — this matches PUC's `LUA_HOOKRET` semantics for
`coroutine.yield` (which also defers the return event until resume).

Re-entrancy contract: hook bodies under async mode may call sync
`vm.eval(...)` but **must not** invoke async natives — the inner call
runs in sync context and hits the existing
`"async native called in sync context"` rejection. The hook callback
type `RustDebugHook = fn(&mut Vm, RustHookEvent)` is a bare function
pointer, so it is unconditionally `Send + Sync` and composes with
SendVm forks (see the `SendVm` section below) without extra trait
bounds. A compile-time `assert_send::<RustDebugHook>()` test pins
this in `crates/luna-core/tests/async_hook_composition.rs`.

## `feature = "send"` — `SendVm` (v1.3+)

v1.3 ships the `send` opt-in feature on luna-core and luna-jit. It
lights up a second public type — [`vm::SendVm`] — that is `Send` at
the type level (clones can move into other OS threads / tokio tasks
and survive across `.await` on a `multi_thread` runtime).

```toml
[dependencies]
luna-core = { version = "1.3", features = ["send"] }
# or
luna-jit  = { version = "1.3", features = ["send"] }
```

```rust
use luna_core::version::LuaVersion;
use luna_core::vm::SendVm;

let vm = SendVm::new(LuaVersion::Lua55);
vm.open_base();
vm.open_math();

// Move into another thread.
std::thread::spawn(move || {
    vm.set_global("x", 41_i64).unwrap();
    let r = vm.eval("return x + 1").unwrap();
    // r[0] == Value::Int(42)
}).join().unwrap();
```

### When to use

- ✅ tokio `multi_thread` runtime, holding the Vm across `.await`
- ✅ Request-per-script web server, where the executor may park the
  task on a different worker after a yield
- ✅ Worker-pool embedding where a queue of Lua jobs dispatches to
  any thread
- ✅ Long-running background script in its own thread spawned from
  a thread that doesn't own the Vm

### When NOT to use

- ❌ Single-thread scripting (game engine main thread, CLI tool,
  REPL) — use plain [`vm::Vm`] for the JIT and zero lock cost
- ❌ Parallel-script throughput — two `SendVm` clones contending
  for the same lock serialize, they don't parallelize. For real
  parallelism, construct one `SendVm` (or one bare `Vm`) per
  worker thread.

### Shape

```rust,ignore
pub struct SendVm {
    inner: Arc<UnsafeCell<Vm>>,
    lock:  Arc<RwLock<()>>,
}

unsafe impl Send for SendVm {}
// Not Sync — cross-thread &SendVm is forbidden; move/clone-and-move only.
```

Every public method takes the lock's write guard before reaching
the inner Vm. The lock is uncontended on the single-worker case
(~10-30 ns per call) and serializes cleanly under contention. Clone
the handle (`SendVm::clone()` is cheap — two `Arc::clone` calls)
and move clones into threads; both clones see the same underlying
Vm.

### Cost

Per the SS-B sign-test bench (macOS M-series, 2026-06-24):

| Workload | Bare interp `Vm` | `SendVm` | Delta |
|---|---|---|---|
| `eval("return 1+2")` | 3.34 µs | 3.27 µs | within noise |
| Token bucket 1k ops | 172.26 µs | 175.46 µs | **+1.86 %** |

Audit-time projection was ~3 % on ARM; we landed at ~2 %. Linux
x86_64 numbers will land via the `gh workflow run perf-gate`
matrix; audit projected ~6 % there.

### Interp-only constraint (v1.3)

`SendVm::new` calls `Vm::new_minimal` internally, which leaves
`JitState` at `NullJitBackend`. **The trace JIT does not run on a
SendVm in v1.3.** This is a documented contract: the Proto-side
trace cache (`Proto::traces: RefCell<Vec<Rc<CompiledTrace>>>`)
intersects with `Send` and migrating it to `Arc<Mutex<Vec<Arc<...>>>>`
is a post-v1.3 polish item (~6 % additional JIT-engaged cost per
audit). Embedders who need both Send semantics and JIT today should
run one bare `Vm` per OS thread and exchange data via channels
(Pattern 3 above).

### Tokio embed pattern (without depending on tokio in luna-core)

luna-core itself doesn't pull tokio (0-dep contract). The shape
embedders ship is:

```rust,ignore
use luna_core::version::LuaVersion;
use luna_core::vm::SendVm;
use std::sync::Arc;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let vm = Arc::new(SendVm::new(LuaVersion::Lua55));
    vm.open_base();

    let mut joins = Vec::new();
    for i in 0..100 {
        let vm = vm.clone();
        joins.push(tokio::spawn(async move {
            // SendVm is Send; .await between calls is fine.
            tokio::task::yield_now().await;
            let r = vm.eval(&format!("return {} + 1", i)).unwrap();
            r
        }));
    }
    for j in joins {
        let _ = j.await.unwrap();
    }
}
```

The lock serializes the 100 tasks; the value is that you can hold
the SendVm across `.await` on the multi_thread runtime (which may
re-park the task on a different worker), not that the tasks run in
parallel.

### API surface

The wrapper mirrors the common embedder ops on [`Vm`]:

- `SendVm::new(version) -> Self`
- `open_base / open_math / open_string / open_table / open_coroutine`
- `eval(src) -> Result<Vec<Value>, LuaError>`
- `call_value(f, args) -> Result<Vec<Value>, LuaError>`
- `set_global<V: IntoValue>(name, v) -> Result<(), LuaError>`
- `get_global(name) -> Value` (new — present only on SendVm)
- `intern_str(s) -> Gc<LuaStr>`
- `set_userdata<T: LuaUserdata>(name, value) -> Result<(), LuaError>`
- `pin_host / read_host / unpin` (Phase SR host roots)
- `Clone` (cheap, shares the underlying Vm via Arc)

Not on `SendVm` (intentionally — these would require additional
Send retrofits across `runtime/`): `register_native_typed` with
non-`Send` closures, `install_jit_backend` (interp-only), the
`debug` library hooks (`set_rust_debug_hook`'s callback is `fn`
which *is* `Send` but the surface isn't mirrored in v1.3 — open
the wrapped Vm via the bare API for now).

For a full design + the soundness story, see
`.dev/rfcs/v1.3-rfc-send-arc.md`.

[`vm::Vm`]: ../crates/luna-core/src/vm/exec.rs
[`vm::SendVm`]: ../crates/luna-core/src/vm/send_vm.rs

---

## Forward-looking — v1.4+ post-ship polish

`SendVm` has two follow-up axes in the pipeline:

1. **JIT-aware `SendVm`** — lift the interp-only restriction. Cost
   sketch in `.dev/rfcs/v1.3-audit-send-vm-design.md` §3.3 + §7.3;
   needs `Proto::traces` `Rc → Arc` migration and a JIT TLS
   redesign. Audit projects ~6 % additional JIT-engaged cost
   beyond the current ~2 % interp-only.
2. **Per-field `SendGc<T>` fork** — replace the wrapper-with-lock
   with a parallel `SendVm` type whose `Gc<T>` is `Arc<UnsafeCell<T>>`
   per-field. Eliminates the per-call lock acquire. Audit §3.2
   B2 estimates ~3 % ARM; SS-B already lands at ~2 % via the
   wrapper, so this is only worth doing if a real embedder hits
   the lock-contention ceiling.

Both are post-v1.3 polish items, not defers — the v1.3 charter is
explicit that the wrapper-shape SendVm is the v1.3 deliverable.

---

## See also

- [`architecture.md`](architecture.md) — Crate layout, JIT pipeline, threading model overview
- [`compatibility.md`](compatibility.md) — Per-dialect feature support
- [`performance.md`](performance.md) — Cross-dialect bench numbers
- `.dev/rfcs/v1.1-rfc-vm-send-sync.md` — Full design rationale for the v1.1 `!Send` stance
- `.dev/rfcs/v1.3-audit-send-vm-design.md` — v1.3 SS audit (per-field fork vs wrapper)
- `.dev/rfcs/v1.3-rfc-send-arc.md` — v1.3 SS-B as-shipped RFC (wrapper design + soundness)
