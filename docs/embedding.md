# Embedding

Cookbook for hosting luna inside a Rust program. Snapshot at v1.1.
Companion docs: [`architecture.md`](architecture.md) (crate layout +
JIT pipeline), [`threading.md`](threading.md) (async + multi-thread
patterns), [`compatibility.md`](compatibility.md) (per-dialect
feature matrix), [`performance.md`](performance.md) (benchmark
numbers).

Each section is independently usable — start at "Hello, world" and
jump around.

---

## 1. Install

luna ships as a Cargo **workspace** with two crates. Pick based on
whether you need the Cranelift JIT:

```toml
# Most embedders — full interpreter + Cranelift JIT + capi.
[dependencies]
luna-jit = "1.1"
```

```toml
# Minimum dep surface — pure interpreter, no Cranelift.
# Embedders running on wasm32, in audited supply chains, or shipping
# small static binaries pick this path.
[dependencies]
luna-core = "1.1"
```

`luna-core` has **zero third-party dependencies** (`cargo tree -p
luna-core` shows exactly one crate — luna-core itself). The full
`luna` crate adds 6 Cranelift crates and their transitive deps.

The CLI binary lives in `luna`:

```sh
cargo install luna-jit  # installs the `luna` REPL + script runner
```

---

## 2. Hello, world

```rust
use luna_jit::vm::Vm;
use luna_jit::version::LuaVersion;

fn main() {
    let mut vm = Vm::new(LuaVersion::Lua55);  // 5.5 + full stdlib + JIT on
    let result = vm.eval("return 'hello, ' .. 'world'").unwrap();
    let s: String = result[0].try_as_str().unwrap().to_string();
    println!("{s}");
}
```

`Vm::new(version)` opens every safe-by-default stdlib library and
installs the Cranelift JIT (when using the `luna` crate; `luna-core`
uses the no-op JIT backend). For finer control see §3.

---

## 3. Sandbox setup

Embedders running untrusted scripts use the sandbox builder. It
opens only the libraries you whitelist, sets the budget and memory
cap, and rejects precompiled bytecode (which bypasses the parser's
safety gates):

```rust
use luna_jit::Lua;  // or `luna_jit::vm::Vm` for the low-level handle
use luna_jit::version::LuaVersion;

let mut lua = Lua::sandbox(LuaVersion::Lua54)
    .open_base()
    .open_math()
    .open_string()
    .open_table()
    .with_instr_budget(1_000_000)   // ~10 ms wall-clock budget
    .with_memory_cap(8 * 1024 * 1024)  // 8 MiB cap
    .build();

// Now run anything you want — the script can't `require`, touch the
// filesystem, or compile bytecode chunks.
let r: i64 = lua.eval("return 1 + 2").unwrap();
```

The builder **omits `io`, `os`, `debug`, `package`** intentionally.
Trusted hosts that need them call `vm.open_io()` / `vm.open_os()` /
etc. on the built `Vm`. See [`compatibility.md`](compatibility.md)
for the per-library feature matrix.

Bytecode loading is **off** by default in the builder. Enable it
with `.allow_bytecode_loading()` only for fully trusted input.

---

## 4. Setting globals

Use `vm.set_global(name, value)` for any [`IntoValue`] type —
integers, floats, booleans, strings, table/function/userdata handles,
`Option<T>`, or `Value` itself:

```rust
vm.set_global("answer", 42_i64)?;
vm.set_global("pi", 3.14_f64)?;
vm.set_global("name", "luna")?;
vm.set_global("ready", true)?;
vm.set_global("missing", Option::<i64>::None)?;  // sets to nil
```

`IntoValue` is implemented for the common Rust primitive types plus
the GC handle types (`Gc<Table>`, `Gc<LuaClosure>`, `Gc<NativeClosure>`).
Embedders adding their own types implement `IntoValue` directly:

```rust
use luna_jit::vm::{IntoValue, Vm};
use luna_jit::runtime::Value;

struct UserId(u64);

impl IntoValue for UserId {
    fn into_value(self, _vm: &mut Vm) -> Value {
        Value::Int(self.0 as i64)
    }
}

// Now:
vm.set_global("uid", UserId(1234))?;
```

---

## 5. Tables

The dogfood §4.1 friction point was building tables. v1.1 ships two
ergonomic paths:

```rust
use luna_jit::runtime::Value;

// One-shot, fixed-size:
let t = vm.table_of([
    ("answer", 42_i64),
    ("year", 2026_i64),
    ("name", "luna"),  // mixed value types — IntoValue covers them
]);
vm.set_global("config", Value::Table(t))?;

// Multi-step builder for variable shapes:
let t = vm.new_table()
    .with("name", "luna")
    .with("major", 1_i64)
    .with("minor", 1_i64)
    .with(1_i64, "first array entry")  // integer keys + mixed values OK
    .with(2_i64, "second array entry")
    .build();
vm.set_global("info", Value::Table(t))?;
```

The builder consumes itself on each `.with(...)` call so chains are
ownership-clean. `.try_with(k, v)` is the fallible variant for
embedders who want `Result` propagation on table overflow (extremely
unlikely in practice — `MAX_ASIZE = 1<<27`).

---

## 6. Native functions

Expose Rust functions to Lua via `vm.native_typed` — the framework
decodes arguments and encodes returns automatically:

```rust
// Pure function:
let add = vm.native_typed(|a: i64, b: i64| -> i64 { a + b });
vm.set_global("add", add)?;

// Multi-return:
let split = vm.native_typed(|x: i64| -> (i64, i64) { (x / 10, x % 10) });
vm.set_global("split", split)?;

// Fallible (use Result<T, LuaError> in the return):
use luna_jit::vm::LuaError;
use luna_jit::runtime::Value;

let safe_div = vm.native_typed(|a: i64, b: i64| -> Result<i64, LuaError> {
    if b == 0 {
        Err(LuaError::new(Value::Nil))  // or build a string error
    } else {
        Ok(a / b)
    }
});
vm.set_global("safe_div", safe_div)?;

// Lua sees:
vm.eval("return add(40, 2)")?;  // -> 42
vm.eval("return split(127)")?;  // -> 12, 7
vm.eval("return safe_div(10, 0)")?;  // -> error
```

**Argument types** can be any `FromLuaValue` impl: `i64`, `f64`,
`bool`, `String`, `Vec<u8>`, `Value`, `Option<T>`. **Return types**
implement `IntoLuaReturn`: any single `IntoValue` type, `()` (zero
returns), tuples up to arity 6, or `Result<T, LuaError>`.

**Arity** is 0-6. Beyond 6, embedders tuple their inputs (`fn(t: (a, b, c, d, e, f, g))`)
or use `vm.native_with(...)` with manual argument decoding.

**Captures**: `vm.native_typed` accepts non-capturing closures and
function pointers (both ZST or pointer-sized). Capturing closures
fall back to `vm.native_with(...)` with explicit upvals.

---

## 7. Userdata — exposing host types

Stash arbitrary `T: 'static` Rust values inside Lua userdata. v1.2
exposes Lua-callable methods + metamethods through the
[`LuaUserdata`] trait; for plain `T: 'static` types without
Lua-side methods, an empty impl is the one-line bridge:

```rust
use luna_core::vm::LuaUserdata;

#[derive(Debug)]
struct DbConn {
    url: String,
    pool_size: u32,
}

impl LuaUserdata for DbConn {}
```

```rust
let conn = DbConn {
    url: "postgres://localhost/app".to_string(),
    pool_size: 8,
};
vm.set_userdata("db", conn)?;

// Host-side read/write:
let c: &DbConn = vm.userdata_borrow("db").unwrap();
println!("pool size: {}", c.pool_size);
let c: &mut DbConn = vm.userdata_borrow_mut("db").unwrap();
c.pool_size = 16;
```

> v1.1 → v1.2 migration: `Vm::create_userdata` / `Vm::set_userdata`
> now require `T: LuaUserdata`. Existing `T: 'static` types upgrade
> with an empty `impl LuaUserdata for T {}`. The metatable produced
> by the trait is auto-installed on the userdata at creation time
> (cached per-Vm by `TypeId::of::<T>()`).

### 7.1 `LuaUserdata` trait — methods + metamethods

```rust
use luna_core::vm::{LuaUserdata, MetaMethod, UserdataMethods};

struct Counter {
    value: i64,
}

impl LuaUserdata for Counter {
    fn type_name() -> &'static str { "Counter" }

    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        // Regular method — `obj:get()`.
        m.add_method("get", |_vm, this, ()| Ok::<_, _>(this.value));

        // Mutating method — `obj:incr(by)`.
        m.add_method_mut("incr", |_vm, this, (by,): (i64,)| {
            this.value += by;
            Ok::<_, _>(())
        });

        // Metamethod — `tostring(obj)`.
        m.add_meta_method(MetaMethod::ToString, |_vm, this, ()| {
            Ok::<_, _>(format!("Counter({})", this.value))
        });
    }
}

vm.set_userdata("c", Counter { value: 100 })?;
vm.eval("c:incr(50); print(tostring(c))")?;   // → Counter(150)
```

The closure shape is `Fn(&mut Vm, &T, A) -> Result<R, LuaError>` for
`add_method` and `Fn(&mut Vm, &mut T, A) -> Result<R, LuaError>` for
`add_method_mut`. `A` is any [`FromLuaArgs`] tuple (0-6 fixed args,
`(T0, T1, …)`) or `Vec<Value>` for variadic dispatch. `R` is any
[`IntoLuaReturn`] (primitives, tuples, `Value`, …). Closures must
be **non-capturing** (`Copy + 'static + ZST`) — capture state by
making it part of `T` itself.

[`LuaUserdata`]: https://docs.rs/luna-core/latest/luna_core/vm/trait.LuaUserdata.html
[`FromLuaArgs`]: https://docs.rs/luna-core/latest/luna_core/vm/trait.FromLuaArgs.html
[`IntoLuaReturn`]: https://docs.rs/luna-core/latest/luna_core/vm/trait.IntoLuaReturn.html

### 7.2 Static constructors via `add_function`

`add_function` registers an entry directly on the metatable (not
under `__index`), so it is callable as a static fn when the
metatable is exposed as a global:

```rust
impl LuaUserdata for Vec3 {
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_function("new", |vm, (x, y, z): (f64, f64, f64)| {
            Ok::<_, _>(vm.create_userdata(Vec3 { x, y, z }))
        });
        m.add_meta_method(MetaMethod::Add, /* ... */);
    }
}

// Expose the metatable as the `Vec3` global so scripts can call
// `Vec3.new(1, 2, 3)`:
let mt = vm.register_userdata::<Vec3>()?;
vm.set_global("Vec3", luna_core::runtime::Value::Table(mt))?;
```

See `crates/luna-jit/examples/userdata_vec3.rs` for the runnable
version with `__add` / `__sub` arithmetic metamethods.

### 7.3 Variadic dispatch (`Vec<Value>`)

For a Redis-style `obj:call(cmd, ...)` dispatcher, take `Vec<Value>`
to collect all positional args:

```rust
m.add_method_mut("call", |vm, this, args: Vec<Value>| {
    // args[0] = cmd; args[1..] = command-specific.
    this.dispatch(vm, args)
});
```

See `crates/luna-jit/examples/userdata_redis_stub.rs` for the
full pattern.

### 7.4 Field-style sugar — v1.2 limitation

`add_field_method_get` exists today as sugar for a 0-arg method
returning a single value. **It uses call-syntax** in v1.2 — `obj:width()`
not `obj.width`. True field-style access (no parentheses) needs
`__index` as a function dispatcher rather than a table; that is a
v1.3 polish item, tracked in the v1.2 release notes.

### 7.5 GC + `__gc` finalizers

A trait-installed `MetaMethod::Gc` metamethod fires **before** the
Rust `Drop` on the boxed `T`. PUC's contract is that `__gc` is
registered for finalization at metatable-set time, not at later
mutations of the metatable; v1.2's auto-install honors this by
calling `check_finalizer_userdata` at `create_userdata` time.

### 7.6 Trait contract reminders

- `T` must be `'static`. Trait-bearing host payloads with `Gc<...>`
  fields are **not** supported in v1.2 — the collector traces the
  metatable but not the boxed payload (`runtime/userdata.rs`).
- Method closures must be ZST (non-capturing). Capture state in `T`.
- During an `add_method_mut` body, do **not** concurrently borrow
  the same userdata payload through another API (e.g. a host-side
  `userdata_borrow_mut("name")` on the same global). The trampoline's
  `&mut T` is exclusive within the call; aliasing is undefined.

For the low-level path, get the `Gc<Userdata>` handle out of
`Value::Userdata(g)` and use `g.as_ptr()` (or the heap-safe
accessors) to inspect/mutate.

---

## 8. Coroutines from Rust

Drive Lua coroutines without going through `coroutine.create` /
`:resume` on the Lua side:

```rust
use luna_jit::runtime::Value;

// Compile a Lua coroutine body:
let body = vm.eval(r#"
    return function()
        coroutine.yield(1)
        coroutine.yield(2)
        return 3  -- terminal return
    end
"#)?[0];

// Create + drive:
let co = vm.create_coroutine(body);

let r1 = vm.resume_coroutine(co, vec![])?;
// r1[0] == Int(1)  — first yield

let r2 = vm.resume_coroutine(co, vec![])?;
// r2[0] == Int(2)  — second yield

let r3 = vm.resume_coroutine(co, vec![])?;
// r3[0] == Int(3)  — terminal return; further resumes error
```

`resume_coroutine` returns `Err(LuaError)` if the body raises or
if `co` isn't a `Value::Coro`.

---

## 9. Debug hooks (Rust-side)

Install a Rust callback that fires on script events without going
through Lua-side `debug.sethook`:

```rust
use luna_jit::vm::Vm;
use luna_jit::vm::exec::{
    RustHookEvent,
    HOOK_MASK_CALL, HOOK_MASK_RETURN, HOOK_MASK_LINE, HOOK_MASK_COUNT,
};

fn observe(_vm: &mut Vm, event: RustHookEvent) {
    match event {
        RustHookEvent::Call => println!("→ function entry"),
        RustHookEvent::Return => println!("← function return"),
        RustHookEvent::Line(n) => println!("• line {n}"),
        RustHookEvent::Count => println!("count event"),
        RustHookEvent::TailCall => println!("→ tail call"),
    }
}

vm.set_rust_debug_hook(
    Some(observe),
    HOOK_MASK_CALL | HOOK_MASK_RETURN,  // pick the events you want
    0,  // count interval (only used with HOOK_MASK_COUNT)
);

// Clear later:
vm.clear_rust_debug_hook();
```

The Rust hook fires synchronously inside the dispatcher; reentrancy
is suppressed (the `in_hook` flag prevents the hook from triggering
itself). Lua-side `debug.sethook` continues to work independently;
both can coexist.

---

## 10. Error handling

Lua errors propagate as `Result<T, LuaError>`. The error value is
the Lua-side `error(...)` argument (usually a string); rich context
lives on the `Vm`:

```rust
use luna_jit::vm::{LuaError, LuaErrorKind};

match vm.eval("error('something failed')") {
    Ok(v) => println!("ok: {:?}", v),
    Err(e) => {
        // Quick: format the LuaError directly:
        println!("error: {}", e);

        // Or get details from the Vm:
        let kind = vm.error_kind();  // -> LuaErrorKind::Runtime
        let source = vm.error_source();  // -> Option<(&str, u32)>
        let traceback = vm.take_error_traceback();  // -> Option<String>
        eprintln!("kind={kind} source={source:?}");
    }
}
```

`LuaErrorKind` classifies common cases:

| variant | when it fires |
|---|---|
| `Runtime` | default — `error(...)`, type errors, missing globals |
| `Syntax` | parser/lexer rejected the source |
| `InstrBudget` | `set_instr_budget` exhausted |
| `MemoryCap` | `set_memory_cap` exceeded |
| `Native` | a native callback returned `Err(LuaError)` |
| `OutOfMemory` | allocation failed |
| `Type` | type mismatch at an arithmetic boundary |

`LuaError` implements `std::fmt::Display` and `std::error::Error`,
so it composes with `?` and the `anyhow` / `thiserror` ecosystem.

---

## 11. The `Lua` newtype facade

`luna_jit::Lua` is an mlua-shape front door:

```rust
use luna_jit::Lua;
use luna_jit::version::LuaVersion;

let mut lua = Lua::new();  // JIT on, Lua 5.5
lua.open_base();
lua.open_math();

// All the IntoValue / FromLuaValue / native_typed plumbing reuses
// the lower-level Vm code.
let add = lua.create_function(|a: i64, b: i64| -> i64 { a + b });
lua.set_global("add", add)?;

let r: i64 = lua.eval("return add(40, 2)")?;
assert_eq!(r, 42);

// Tables as first-class handles:
let t = lua.create_table();
t.set(&mut lua, "name", "luna")?;
t.set(&mut lua, "year", 2026_i64)?;
let name: String = t.get(&mut lua, "name")?;

// Globals via the same handle interface:
let g = lua.globals();
let answer: i64 = g.get(&mut lua, "answer")?;
```

Handles (`LuaFunction`, `LuaTable`, `LuaRoot`) are `Copy + Clone`
and survive across calls. The host root pool is append-only;
release the whole pool with `lua.unpin_all()` between batches
(slot recycling lands in Phase 4+).

For escape-hatch access to the underlying `Vm`:

```rust
let vm: &mut Vm = lua.vm();
```

---

## 12. Threading model

`Vm` (and `Lua`) is **`!Send + !Sync`**. One Vm lives on the OS
thread that created it. Embedders wanting concurrency spawn one
Vm per worker; async embedders pin Vm access to a single-thread
Tokio runtime or a `LocalSet`. See [`threading.md`](threading.md)
for the canonical patterns (single-thread Tokio, `LocalSet`,
per-OS-thread `Vm` + channels) and the post-v1.1
`feature = "send"` roadmap.

The constraint is type-system-enforced; a compile_fail doctest on
`Vm` catches accidental loss of the `!Send` invariant.

---

## Where to go next

- [`architecture.md`](architecture.md) — crate layout, JIT pipeline,
  source classification
- [`threading.md`](threading.md) — async + multi-thread embedding
- [`compatibility.md`](compatibility.md) — per-dialect feature
  matrix (Lua 5.1 / 5.2 / 5.3 / 5.4 / 5.5)
- [`performance.md`](performance.md) — cross-dialect + Redis-Lua
  bench numbers
- `cargo doc --open` — full API reference for every public type
