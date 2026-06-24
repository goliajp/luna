# luna-aot

Ahead-of-time compiler from Lua source to a self-contained native
binary, built on `luna-core`'s VM. Sibling of `luna-core` (pure-interp
runtime, zero third-party deps) and `luna-jit` (the runtime
Cranelift JIT).

> **v1.3 scaffold session — pipeline shape only.** End-to-end today =
> Lua source → luna bytecode dump → ELF/Mach-O/PE data section → linked
> native binary that prints the embedded section size. The interp-driven
> Vm dispatch wiring and the Cranelift-lowered trace mcode emission are
> follow-up sessions within the v1.3 mega sprint (see
> `.dev/rfcs/v1.3-audit-luna-aot.md`).

## Quick start (host triple only at scaffold)

```sh
cargo run -p luna-aot -- compile foo.lua --out foo
./foo
# stderr: luna-aot scaffold: embedded bytecode length = ... bytes (section .luna.bytecode)
```

## Why a separate crate

`luna-aot` pulls third-party deps (`object`, `clap`, and — in the
follow-up codegen sessions — all of `cranelift`). Keeping it sibling
to `luna-jit` lets embedders pick exactly one of:

- **`luna-core`** — pure interp, zero third-party deps.
- **`luna-core + luna-jit`** — runtime JIT (mmap RWX, recorded traces).
- **`luna-core + luna-aot`** — offline compile to a deployable binary;
  the produced binary statically links `luna-core` only (no
  `luna-jit`, no `luna-aot`, no Cranelift dynamic link).

The `luna-core` zero-third-party-dep contract is **unaffected** by
this crate.

## Pipeline (scaffold today, audit § Stages 1-6 long-term)

```text
foo.lua
  │ luna_core::frontend::parser::parse
  ▼
Chunk (AST)
  │ luna_core::compiler::compile_chunk
  ▼
Gc<Proto>
  │ luna_core::vm::dump::dump  (luna's own body format)
  ▼
Vec<u8>
  │ object::write::Object  (.luna.bytecode section + bracket symbols)
  ▼
foo.luna_bytecode.o   ELF / Mach-O / PE
  │ cc foo.luna_bytecode.o foo.luna_stub.o -o foo
  ▼
foo   (today: scaffold entry prints len; follow-up: real Vm runtime)
```

## Scope this session

| Stage | Status |
|---|---|
| 1. parse → AST                   | reused from luna-core, no new code |
| 2. AST → Proto                   | reused from luna-core, no new code |
| 3. Proto → Cranelift IR (lowerer refactor) | **deferred** — follow-up |
| 4. emit `.o` via `cranelift-object::ObjectModule` | **deferred** — follow-up; today's `.o` only carries bytecode, not native mcode |
| 5. embed bytecode + source as data sections | **bytecode done** (`.luna.bytecode` + `__luna_bytecode_{start,end}`); source-embed deferred |
| 6. link via system `cc`          | scaffold C entry done; real Rust runtime stub link is follow-up |

See `.dev/rfcs/v1.3-audit-luna-aot.md` for the full 70-day audit
(crate scaffold → trace-everything pipeline → Lua source embed +
interp fallback → linker invocation → examples → testing). Effort
estimate breakdown is in § "Effort breakdown".

## CLI surface

```text
luna-aot compile <input.lua> [--out <path>] [--target <triple>] [--dialect <5.X>]
```

`--target` rejects anything other than the host triple in the
scaffold; cross-compile lands with the Stage 6 follow-up.

`--dialect` accepts `5.1` / `5.2` / `5.3` / `5.4` / `5.5` / `macrolua`;
default is `5.5`.

## Status — supply-chain delta

| crate | direct deps added |
|---|---|
| `luna-aot` (new) | `luna-core` (workspace) + `object 0.36` + `clap 4` (+ `tempfile` dev-only) |
| `luna-core`      | **none** — 0-third-party-dep contract preserved |
| `luna-jit`       | **none** |
| `luna-jit-derive`| **none** |

CI `zero-dep` job continues to report luna-core's dep tree as one
row (itself).
