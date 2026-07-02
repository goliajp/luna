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

The default install keeps the `luna` binary minimal: stdin REPL with
multi-line continuation + `~/.luna_history`, no extra deps beyond
Cranelift. For an interactive editor with **tab completion against
your `Vm` globals** and **Lua syntax highlighting**, opt into the
`repl-line-editor` feature (pulls `rustyline` and friends; luna-core
stays 0-dep regardless):

```sh
cargo install luna-jit --features repl-line-editor
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
Trusted hosts that need them call `vm.open_os_io()` (`io.*` + `os.*`
together) / `vm.open_debug()` / `vm.open_package()` on the built
`Vm`. See [`compatibility.md`](compatibility.md) for the per-library
feature matrix and [`security.md`](security.md) for the threat model
covering each opt-in.

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

### 7.4 Field-style sugar (v1.3 UD1+UD2)

`add_field_method_get(name, fn)` and the new
`add_field_method_set(name, fn)` register accessors for true field-style
syntax:

```rust
impl LuaUserdata for Box2 {
    fn add_methods<M: UserdataMethods<Self>>(m: &mut M) {
        m.add_field_method_get("width", |_vm, this| Ok::<_, _>(this.width));
        m.add_field_method_set("width", |_vm, this, (w,): (i64,)| {
            this.width = w;
            Ok(())
        });
    }
}
```

```lua
print(b.width)   -- 16
b.width = 100
print(b.width)   -- 100
b.unknown = 1    -- error: attempt to write unknown field 'unknown' on Box2
```

Implementation: when any getter is registered, the metatable's `__index`
slot becomes a native trampoline that dispatches `methods → field-
getters → nil`; when any setter is registered, the metatable's
`__newindex` slot becomes a trampoline that dispatches to the setter
or raises `attempt to write unknown field …` (no silent fallback,
per `code/no-unsolicited-fallback`).

**v1.2 → v1.3 breaking change**: in v1.2, `add_field_method_get`
generated a method-table entry, so `obj:width()` (call-syntax) worked.
In v1.3 the entry is dispatched through the function-`__index`
trampoline, so `obj.width` returns the field value directly and
`obj:width()` no longer works for getters defined this way (calling a
returned `Int(16)` errors). Embedders who need both shapes should
register an explicit `add_method("width", ...)` alongside the
field-getter. Methods win over field-getters on name collision
(matches mlua; precedence is documented in
`crates/luna-core/tests/userdata_trait.rs::methods_win_on_collision`).

### 7.4a `#[derive(LuaUserdata)]` proc-macro (v1.3 UD3)

For ≥5-method userdata types the builder calls become repetitive; the
new `luna-jit-derive` crate ships a derive that emits the trait impl
for you:

```rust
use luna_jit::{LuaUserdata, lua_userdata_methods};
use luna_core::vm::{LuaError, Vm};

#[derive(LuaUserdata)]
#[lua_type_name = "Counter"]
struct Counter { value: i64 }

#[lua_userdata_methods]
impl Counter {
    #[lua_method("get")]
    fn get(&self, _vm: &mut Vm, _: ()) -> Result<i64, LuaError> {
        Ok(self.value)
    }

    #[lua_method_mut("incr")]
    fn incr(&mut self, _vm: &mut Vm, (by,): (i64,)) -> Result<(), LuaError> {
        self.value += by;
        Ok(())
    }
}
```

Helper attrs (placed on `fn` items inside the impl block):

| Attribute | Lowers to |
|---|---|
| `#[lua_method("name")]` | `add_method` |
| `#[lua_method_mut("name")]` | `add_method_mut` |
| `#[lua_function("name")]` | `add_function` (no receiver) |
| `#[lua_meta_method(Add)]` | `add_meta_method(MetaMethod::Add, …)` |
| `#[lua_meta_method_mut(Concat)]` | `add_meta_method_mut` |
| `#[lua_field_get("name")]` | `add_field_method_get` |
| `#[lua_field_set("name")]` | `add_field_method_set` |
| `#[lua_skip]` | keep the fn as a pure-Rust helper |

`luna-jit-derive` lives downstream of `luna-core` so the 0-dep
contract is preserved — luna-core embedders who want the derive can
either upgrade to `luna-jit`, or add `luna-jit-derive = "1.3"` as a
direct dep alongside `luna-core` (the derive emits fully-qualified
`::luna_core::vm::*` paths and has no other runtime deps). Hand-impl
remains the supported escape hatch for generic types, conditional
method sets, and `MSRV`-sensitive embedders avoiding `syn` 2.

Add the dep:

```toml
luna-jit = "1.3"
# or
luna-core = "1.3"
luna-jit-derive = "1.3"
```

### 7.5 GC + `__gc` finalizers

A trait-installed `MetaMethod::Gc` metamethod fires **before** the
Rust `Drop` on the boxed `T`. PUC's contract is that `__gc` is
registered for finalization at metatable-set time, not at later
mutations of the metatable; v1.2's auto-install honors this by
calling `check_finalizer_userdata` at `create_userdata` time.

### 7.6 Trait contract reminders

- `T` must be `'static`.
- Method closures must be ZST (non-capturing). Capture state in `T`.
- During an `add_method_mut` body, do **not** concurrently borrow
  the same userdata payload through another API (e.g. a host-side
  `userdata_borrow_mut("name")` on the same global). The trampoline's
  `&mut T` is exclusive within the call; aliasing is undefined.
- If `T` carries a `Gc<...>` field, override [`LuaUserdata::trace`]
  to mark it — see §7.7. The default `trace` is a no-op, suitable
  for pure host types (no Gc-managed inner state).

For the low-level path, get the `Gc<Userdata>` handle out of
`Value::Userdata(g)` and use `g.as_ptr()` (or the heap-safe
accessors) to inspect/mutate.

### 7.7 Trace-bearing host payloads (v1.3+)

When `T` stashes a `Gc<Table>` / `Gc<LuaStr>` / `Gc<NativeClosure>` /
`Gc<Coro>` / `Gc<Userdata>` inside its fields, the collector cannot
discover those handles by walking the `Box<dyn Any>` payload alone —
the `Any` vtable has no "trace" entry. The embedder declares the
reachable Gc set by overriding `LuaUserdata::trace`:

```rust
use luna_core::runtime::{Gc, Table};
use luna_core::vm::{LuaUserdata, UserdataMarker};

struct Cache {
    entries: Gc<Table>,
}

impl LuaUserdata for Cache {
    fn type_name() -> &'static str { "Cache" }

    fn trace(&self, m: &mut UserdataMarker) {
        m.mark(self.entries);
    }
}
```

`UserdataMarker` exposes two methods:

- `mark<T>(&mut self, g: Gc<T>) -> bool` — mark a typed Gc handle.
- `mark_value(&mut self, v: Value) -> bool` — mark every Gc-managed
  object behind a `Value` (no-op for primitives like `Int` / `Bool`).

For container fields, walk them and call `mark` per element:

```rust
struct Pool { tables: Vec<Gc<Table>> }
impl LuaUserdata for Pool {
    fn trace(&self, m: &mut UserdataMarker) {
        for &t in &self.tables { m.mark(t); }
    }
}
```

Contract inside `trace`:

- The call runs synchronously inside the collector's mark phase.
- The embedder may **only** read `&self` and call `mark` / `mark_value`.
- The embedder must **not** allocate new GC objects, reenter the `Vm`,
  acquire locks, or perform I/O.
- The default `trace` is a no-op, so existing v1.1 / v1.2 types with
  empty `impl LuaUserdata for T {}` keep compiling and running
  unchanged.

Forgetting to override `trace` when `T` carries a `Gc<...>` field
whose lifetime isn't otherwise rooted (via `Vm::pin_host` or a
Lua-side table) risks dangling references after the next GC cycle.
The contract is on the embedder; the runtime does not detect it.

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
and survive across calls. They wrap an opaque
`luna_core::vm::HostRootTicket` (8 bytes) that the underlying
slot-recycling pool issues at `pin_host` time.

**v1.3 Phase SR — slot recycling**: long-running embedders
(request-per-script loops, edge workers) release individual handles
via `lua.unpin(handle)`; the slot is recycled for the next pin so
the pool size stays bounded. The whole batch can still be released
in one shot via `lua.unpin_all()`. Both operations bump the slot's
generation; using a stale `LuaFunction` / `LuaTable` / `LuaRoot`
after release panics with `"<HandleType> used after unpin /
unpin_all"`.

```rust
// Request-per-script loop — pool stays at ≤ N slots regardless of
// loop count.
loop {
    let t = lua.create_table();
    t.set(&mut lua, "name", "request")?;
    // ... use t ...
    lua.unpin(t)?;  // single-handle release; slot recycled
}
```

Embedders authoring their own facade (parallel to `LuaFunction`)
work with the raw `Vm` API: `vm.pin_host(v) -> HostRootTicket`,
`vm.read_host(t) -> Option<Value>`, `vm.write_host(t, v) ->
Result<(), HostRootStale>`, `vm.unpin(t) -> Result<(),
HostRootStale>`. The `HostRootStale` error type carries the
ABA-detection semantics — stale tickets (held across an
unpin/re-pin on the same slot) read as `None` rather than
silently returning the new slot's unrelated value.

**Migration from v1.1 / v1.2**: callers stashing the bare `usize`
from `Vm::pin_host` move to `HostRootTicket`; `host_root_at(idx)`
and `host_root_set(idx, v)` (panic on OOB) are removed in favor of
`read_host(t)` (returns `Option<Value>`) and `write_host(t, v)`
(returns `Result<(), HostRootStale>`).

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

## 13. Stable API contract (v3.0 acceptance #7)

Starting at v2.7.0 (2026-07-01) luna's public API is
partitioned into a **stable** surface (SemVer-major to break)
and an **unstable / internal** surface (may break in minor
releases). Source-of-truth for the classification is the
v2.7 API audit (private dev material). The public contract:

### Stable — SemVer-major to break

- `luna_jit`'s front-door types: `Lua`, `LuaFunction`,
  `LuaTable`, `LuaRoot`, `LuaSandboxBuilder`, `IntoLuaArgs`
- Constructor entry points: `new_with_jit`,
  `new_minimal_with_jit`, `install_default_jit`
- Derive + attr macros: `LuaUserdata`, `lua_userdata_methods`
- `VmExt` trait for advanced embedders
- `luna_core` transitively re-exported via `luna_jit::*`:
  `Vm`, `LuaVersion`, `Value`, `LuaError`, `Vm::new`,
  `Vm::eval`, `Vm::set_memory_cap`,
  `Vm::set_print_handler`, `Vm::host_roots` accessors
- `luna_aot::{BYTECODE_START_SYMBOL, BYTECODE_END_SYMBOL,
  BYTECODE_SECTION_NAME}` (AOT ABI constants); `cli`,
  `embed` modules
- `luna_runtime_helpers::{run_bytecode, force_link_*}`;
  `aot_*_resolver` modules (AOT metadata ABI)

### Unstable / internal — may break in minor

- Deep-nested pub modules: `luna_core::{compiler,
  frontend, jit, pattern}` (crate-internal reasons)
- `luna_jit::{capi, jit_backend, inspect, jit}` (backend
  internals + experimental C ABI)
- `luna_aot::runtime_stub` (test scaffold)

If your embedder currently reaches into an unstable surface,
please file an issue — that's the signal we need to
promote the symbol into the stable set for future
compatibility.

The 6-month stability clock (v3.0 acceptance #7) starts at
v2.7.0 = 2026-07-01. Any stable-surface break resets the
clock. v3.0 ship target ≥ 2027-01-01 at earliest.

---

## 14. Known limitations

*(none currently)*

**Resolved in v2.13** — Windows gc.lua / gengc.lua /
tracegc.lua weak-table sweep `STATUS_ACCESS_VIOLATION`: root-
caused to two platform-independent GC bugs (a stale stack-root
cursor on `collectgarbage()` calls, and weak-table tombstone
keys escaping the clear-key sweep), both fixed and validated
with ASAN + a new `gc-verify` invariant-checking build plus
repeated Windows stress runs. The official-suite Windows gate
is removed; all three files run unconditionally on every
platform.

---

## Where to go next

- [`architecture.md`](architecture.md) — crate layout, JIT pipeline,
  source classification
- [`threading.md`](threading.md) — async + multi-thread embedding
- [`compatibility.md`](compatibility.md) — per-dialect feature
  matrix (Lua 5.1 / 5.2 / 5.3 / 5.4 / 5.5)
- [`performance.md`](performance.md) — cross-dialect + Redis-Lua
  bench numbers
- [`embedder-recruitment.md`](embedder-recruitment.md) — luna is
  actively looking for its second production embedder to close v3.0
  acceptance #8; a short overview of what luna offers, what it
  doesn't, and how to try it
- `cargo doc --open` — full API reference for every public type
