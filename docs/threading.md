# Threading

How to use luna in async and multi-threaded host programs. Snapshot
at v1.1.

luna's `Vm` is **`!Send + !Sync`** — a `Vm` (and any handle into its
GC heap) lives on the OS thread that created it. This page covers
the canonical embedding patterns and the reasoning behind the
constraint.

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
let vm = luna::Vm::new_minimal_with_jit(LuaVersion::Lua55);
tokio::spawn(async move {
    vm.eval("return 1").await  // error[E0277]: `Vm` cannot be sent between threads safely
});
```

luna's `Vm` carries a compile-time `compile_fail` doctest enforcing
this; if a future code change accidentally made `Vm: Send`, the
build fails.

For the planned post-v1.1 `feature = "send"` work, see §[Forward-looking](#forward-looking-feature--send).

---

## Embedding patterns

### Pattern 1 — Single-thread Tokio runtime

Simplest case. `current_thread` flavor runs the executor on one OS
thread; `Vm` lives there.

```rust
use luna::Vm;
use luna::version::LuaVersion;

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
use luna::Vm;
use luna::version::LuaVersion;
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
use luna::Vm;
use luna::version::LuaVersion;
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
use luna::Lua;
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
[`examples/async_host.rs`](../crates/luna/examples/async_host.rs)
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
- Hook firing for async natives is deferred to Phase 4+; sync
  natives + Lua hooks work normally.

## Forward-looking — `feature = "send"`

A post-v1.1 sprint will introduce a `luna-core/Cargo.toml` feature:

```toml
[features]
default = []
send = []  # opt-in: Vm: Send (cost: ~5-15% perf)
```

When enabled, `Gc<T>` flips from `NonNull<T>` to `Arc<RwLock<T>>`,
the trace-JIT `Rc<CompiledTrace>` side-table becomes `Arc<...>`,
and the heap mark-sweep walk becomes locking. `Vm` would then be
`Send` (still not `Sync` — one mutable Vm per thread is still the
contract).

Hard gates on the future sprint:
- ≤ 8% regression on `cross_dialect` + `lua_microbench` + Redis-Lua-shape
- Full test suite green under `--features send`
- JIT trace path re-validated on aarch64 + x86-64
- capi remains single-threaded (the C ABI side doesn't change)

If the perf budget isn't reachable, the sprint produces a refined
RFC rather than a partial merge. Partial Send-mode is a perf-disaster
trap.

The doc-level enforcement (rustdoc `!Send` note, `compile_fail`
doctest) on `Vm` will continue to track whichever default the crate
ships with at the time. Embedders pinning to `default-features = false`
get today's behavior indefinitely.

---

## See also

- [`architecture.md`](architecture.md) — Crate layout, JIT pipeline, threading model overview
- [`compatibility.md`](compatibility.md) — Per-dialect feature support
- [`performance.md`](performance.md) — Cross-dialect bench numbers
- `.dev/rfcs/v1.1-rfc-vm-send-sync.md` — Full design rationale for the v1.1 `!Send` stance and the post-v1.1 Send sprint
