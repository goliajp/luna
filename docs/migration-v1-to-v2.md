# Migration: luna v1.x → v2.x / v3.x

The consolidated guide for embedders moving off any v1.x release.

**There was never a v2.0.** The v2.0 charter's fourteen tracks shipped
as one release, and that release was **2.1.0** (2026-06-28, 235 commits
after 1.3.0). No `v2.0.0` tag or crates.io version exists. If you are
looking for "what broke in 2.0", the answer is: what broke in 2.1.0,
listed below.

**3.0 broke nothing.** The major bump marks a maturity gate, not an API
break — `luna_core`'s public surface is identical to 2.18.0, and code
that builds against any 2.x builds against 3.0 unchanged. Everything on
this page is about the 1.x → 2.x step.

---

## What does not change

Even across the major bump, these hold:

- **`luna-core` has zero third-party dependencies.** CI-enforced on
  every commit.
- **No `unsafe` at the embedder surface.** The four `pub unsafe fn` in
  the workspace remain `#[doc(hidden)]`.
- **`Vm` is `!Send + !Sync`.** `feature = "send"` stays opt-in; the
  default `Vm` is single-threaded.
- **Sandbox by default.** `Vm::sandbox(...)` opens zero libraries and
  rejects bytecode loading unless told otherwise.
- **Every dialect.** 5.1 / 5.2 / 5.3 / 5.4 / 5.5, plus MacroLua.

---

## 1. Host roots are tickets, not indices

The pin pool used to hand back a `usize` index that stayed valid
forever, which meant a long-running embedder's pool only ever grew.
It now returns a generation-checked ticket, so slots are reusable and a
stale read is caught instead of silently returning someone else's value.

```rust
// v1.x
let idx: usize = vm.pin_host(value);
let v = vm.host_root_at(idx);
vm.host_root_set(idx, new_value);

// v2.x
let ticket: HostRootTicket = vm.pin_host(value);
let v = vm.read_host(ticket).expect("still pinned");
vm.write_host(ticket, new_value)?;      // Err(HostRootStale) if unpinned
vm.unpin(ticket)?;                      // release one slot
```

`vm.unpin_all()` and `vm.host_root_count()` keep their signatures.
`unpin_all` now bumps every slot's generation, so all outstanding
tickets go stale together.

**Recipe:** replace each stored `usize` with a `HostRootTicket`;
`host_root_at(i)` → `read_host(t)`, `host_root_set(i, v)` →
`write_host(t, v)`.

## 2. Facade handles carry a ticket

`LuaFunction`, `LuaTable` and `LuaRoot` hold a `HostRootTicket` where
they held a `usize`. They stay `Copy + Clone` and their method surface
(`call`, `call_multi`, `get`, `set`, …) is unchanged, so this is
invisible unless you constructed or destructured them by field.

New: `Lua::unpin(handle)` releases a single handle, via the
`PinnedHandle` trait implemented by all three. Reading a handle after
`unpin` or `unpin_all` panics with `"<HandleType> used after unpin /
unpin_all"` — the behaviour the v1.1 docstring already described.

## 3. Userdata needs the trait

`Vm::create_userdata` and `Vm::set_userdata` tightened from
`T: Any + 'static` to `T: LuaUserdata`, so a userdata type can be given
a metatable and a `__gc` finalizer at allocation time.

```rust
// v2.x — one line for a type that needs no methods
impl LuaUserdata for MyType {}
```

If the type holds any `Gc<…>` field you must also override `trace` and
mark every handle, or the collector will free something still reachable.
`#[derive(LuaUserdata)]` from `luna-jit-derive` does this for you.

Related, from v1.2: method-call syntax (`obj:width()`) needs an explicit
`add_method` — the dual registration that used to make it work
implicitly was removed.

## 4. `feature = "send"` compiles now

In v1.2 selecting `send` raised a `compile_error!`. It now compiles and
surfaces `SendVm`. If you were guarding against that with
`cfg(not(feature = "send"))`, drop the guard.

---

## A semver violation to know about, inside 2.x

`luna_core::numeric::num_to_string_for` changed signature in **2.14.0**,
a *minor* release. `luna_core::numeric` is public, so this was inside
the stability contract and should not have happened in a minor. The
2.14.0 changelog entry mentioned the new type but never said the
signature moved, and carried no `BREAKING` marker.

```rust
// <= 2.13.0
pub fn num_to_string_for(n: Num, legacy_float: bool) -> String
// >= 2.14.0
pub fn num_to_string_for(n: Num, fmt: FloatFmt) -> String
```

| Old call | New call | Dialects |
|---|---|---|
| `num_to_string_for(n, true)` | `num_to_string_for(n, FloatFmt::Legacy14)` | 5.1, 5.2 (`%.14g`, no `.0`) |
| `num_to_string_for(n, false)` | `num_to_string_for(n, FloatFmt::TwoStage55)` | 5.5 (`%.15g` → round-trip → `%.17g`) |
| *(not expressible)* | `num_to_string_for(n, FloatFmt::G14)` | 5.3, 5.4 (`%.14g` + `.0`) |

The third row is why the change had to happen: 2.13 applied the 5.5
scheme to 5.3/5.4 as well, one of the divergences 2.14 fixed. A `bool`
cannot express three formats, so no compatibility shim was added —
restoring the old overload would permanently carry an API that cannot
say what it does. `FloatFmt` has no constructor from a `LuaVersion`;
pick the variant directly, as the VM does.

It was found — and written down, in the 2.17.0 changelog entry — by the
public-surface audit that now runs every release, which exists because
of this. A minor bump showing a non-empty removal
list fails the release checklist.

---

## Bytecode

luna's own dump format carries no version field, and no release has
recorded or tested whether 1.x-produced bytecode loads under 2.x. Treat
it as unsupported and recompile from source; if you have a case that
depends on it, say so and it can be given a contract and a test rather
than an assumption.

PUC-format `.luac` loading is a separate, opt-in path
(`Vm::set_puc_bytecode_loading(true)`), covered by the differential
corpus on all five dialects.

## See also

- [`CHANGELOG.md`](../CHANGELOG.md) — canonical per-release record;
  every entry above links back to it
- [`embedding.md`](embedding.md) — the current cookbook
- [`threading.md`](threading.md) — `Vm` threading constraints and
  `SendVm`
