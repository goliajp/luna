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

Stash arbitrary `T: 'static` Rust values inside Lua userdata:

```rust
use luna_jit::vm::Vm;

#[derive(Debug)]
struct DbConn {
    url: String,
    pool_size: u32,
}

let conn = DbConn {
    url: "postgres://localhost/app".to_string(),
    pool_size: 8,
};
vm.set_userdata("db", conn)?;

// Later, on the host side:
let c: &DbConn = vm.userdata_borrow("db").unwrap();
println!("pool size: {}", c.pool_size);

// Mutable variant:
let c: &mut DbConn = vm.userdata_borrow_mut("db").unwrap();
c.pool_size = 16;
```

The script sees `db` as a regular `userdata` value (`type(db) ==
"userdata"`). Attach a metatable for method-style dispatch (see PUC
manual §2.4):

```rust
// In a real embedder, you'd register `__index` etc. on the userdata's
// metatable using `set_metatable`. v1.1 ships the raw infrastructure;
// a LuaUserdata trait with sugared method registration lands in v1.2.
```

For the low-level path, get the `Gc<Userdata>` handle out of `Value::Userdata(g)`
and use `g.as_ptr()` (or the heap-safe accessors) to inspect/mutate.

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
