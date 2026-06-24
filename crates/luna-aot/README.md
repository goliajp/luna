# luna-aot

Ahead-of-time compiler from Lua source to a self-contained native
binary, built on `luna-core`'s VM. Sibling of `luna-core` (pure-interp
runtime, zero third-party deps) and `luna-jit` (the runtime
Cranelift JIT).

End-to-end today: Lua source → luna bytecode dump → ELF/Mach-O/PE
data section → linked native binary that **constructs a `Vm` at
process start, undumps the bytecode, and runs it through the
interpreter**. Stage 1-2 (parse/compile), Stage 3 (backend-agnostic
lowerer), Stage 4 (linker + runtime staticlib), Stage 5 (cross-compile
via `--target`), and Stage 6 (Alpine no-Lua deploy smoke) are all
landed. Trace JIT mcode emission via `cranelift-object` is the only
remaining sub-stage and lands as a follow-up.

## Quick start (host triple, single-binary deploy)

```sh
cargo run -p luna-aot -- compile foo.lua --out foo
./foo
# prints whatever `print(...)` calls in foo.lua produced
```

The output is **one file**: no `.so`, no `.dll`, no `liblua*` runtime
dep. `ldd foo` shows only libc + libm + libpthread + (on macOS) the
System framework. `strip foo` still leaves a working executable.

## Cross-compile

`--target <triple>` flows through the full pipeline:

- `cargo build --target=<triple> -p luna-runtime-helpers --release`
  (requires `rustup target add <triple>`)
- target-aware `object` write (ELF / Mach-O / PE magic per OS)
- per-triple `cc` driver pick (`aarch64-linux-gnu-gcc`,
  `x86_64-w64-mingw32-gcc`, `x86_64-linux-musl-gcc`, ...) — falls
  back to `cc -target <triple>` when no named cross-cc is on PATH
- per-OS lib set (`-lpthread`/`-ldl`/`-lm` on glibc Linux, skip
  `-lgcc_s`/`-lutil` on musl, `-luserenv`/`-lws2_32`/`-lbcrypt` on
  Windows-MinGW, `-framework CoreFoundation` on macOS)

### Recipes

```sh
# darwin x86_64 from darwin aarch64 (Apple clang handles -target natively):
luna-aot compile foo.lua --out foo.x86_64 --target x86_64-apple-darwin

# linux aarch64 from any glibc host (needs gcc-aarch64-linux-gnu):
sudo apt install gcc-aarch64-linux-gnu                            # debian/ubuntu
luna-aot compile foo.lua --out foo.arm64 --target aarch64-unknown-linux-gnu

# windows x86_64 from any unix host (needs mingw-w64):
sudo apt install gcc-mingw-w64-x86-64                             # debian/ubuntu
brew install mingw-w64                                            # macOS
luna-aot compile foo.lua --out foo.exe --target x86_64-pc-windows-gnu

# Alpine / musl deploy (single binary that runs on any musl distro):
brew install FiloSottile/musl-cross/musl-cross                    # macOS
sudo apt install musl-tools                                       # debian/ubuntu
luna-aot compile foo.lua --out foo.musl --target x86_64-unknown-linux-musl
```

### Per-target prerequisites

| Triple | Rust target | C cross-compiler | macOS install | Debian install |
|---|---|---|---|---|
| `x86_64-apple-darwin` | rustup add | system clang (apple) | preinstalled | — (darwin SDK needed) |
| `aarch64-apple-darwin` | rustup add | system clang (apple) | preinstalled | — (darwin SDK needed) |
| `aarch64-unknown-linux-gnu` | rustup add | `aarch64-linux-gnu-gcc` | `brew install aarch64-elf-gcc` | `apt install gcc-aarch64-linux-gnu` |
| `x86_64-unknown-linux-gnu` | rustup add | `x86_64-linux-gnu-gcc` | `brew install x86_64-elf-gcc` | (host gcc) |
| `x86_64-unknown-linux-musl` | rustup add | `x86_64-linux-musl-gcc` | `brew install FiloSottile/musl-cross/musl-cross` | `apt install musl-tools` |
| `x86_64-pc-windows-gnu` | rustup add | `x86_64-w64-mingw32-gcc` | `brew install mingw-w64` | `apt install gcc-mingw-w64-x86-64` |
| `x86_64-pc-windows-msvc` | rustup add | **link.exe (MSVC)** — not driven by luna-aot; use `--target x86_64-pc-windows-gnu` instead | n/a | n/a |

Anything missing surfaces as a concrete error message naming the
package; nothing is silently degraded.

## Why a separate crate

`luna-aot` pulls third-party deps (`object`, `clap`, and — in the
follow-up codegen sessions — all of `cranelift`). Keeping it sibling
to `luna-jit` lets embedders pick exactly one of:

- **`luna-core`** — pure interp, zero third-party deps.
- **`luna-core + luna-jit`** — runtime JIT (mmap RWX, recorded traces).
- **`luna-core + luna-aot`** — offline compile to a deployable binary;
  the produced binary statically links `luna-core` + `luna-runtime-helpers`
  only (no `luna-jit`, no `luna-aot`, no Cranelift dynamic link).

The `luna-core` zero-third-party-dep contract is **unaffected** by
this crate.

## Pipeline (Stages 1-6)

```text
foo.lua
  │ luna_core::frontend::parser::parse                       (Stage 1)
  ▼
Chunk (AST)
  │ luna_core::compiler::compile_chunk                       (Stage 2)
  ▼
Gc<Proto>
  │ [trace JIT mcode emission — follow-up]                   (Stage 3)
  │ luna_core::vm::dump::dump  (luna's own body format)
  ▼
Vec<u8>
  │ object::write::Object  (.luna.bytecode + bracket symbols, target-aware)
  ▼                                                          (Stages 4-5)
foo.luna_bytecode.o   ELF / Mach-O / PE
  │ cc bytecode.o cmain.o libluna_runtime_helpers.a -o foo
  ▼                                                          (Stage 6)
foo   single-binary, runs through luna_aot_run → Vm → call_value
```

`luna_aot_run` lives in `crates/luna-runtime-helpers/` as a dual
`staticlib + rlib`. The staticlib carries rust stdlib + all of
luna-core; the rlib is what `luna-aot`'s integration tests link
against so they can drive the same code path in-process without
shelling out to `cc`.

## Stage matrix

| Stage | Status |
|---|---|
| 1. parse → AST                   | shipped — reused from luna-core |
| 2. AST → Proto                   | shipped — reused from luna-core |
| 3. Proto → Cranelift IR (shared lowerer over `M: Module`) | shipped — `luna-jit::jit_backend` lowerers are generic over `cranelift_module::Module`; trace mcode emission via `cranelift-object` is the only follow-up |
| 4. emit `.o` via `object::write::Object` | shipped — bytecode `.o` + C main `.o` |
| 5. embed bytecode + cross-compile via `--target` | shipped — bytecode embed + target-aware ELF/Mach-O/PE magic + per-triple cc + per-OS lib set |
| 6. link + Alpine no-Lua smoke    | shipped — `cargo build -p luna-runtime-helpers --release [--target T]` bootstrap + final `cc` link; Alpine smoke test skips cleanly when musl cross-cc / docker missing |

See `.dev/rfcs/v1.3-audit-luna-aot.md` for the full audit.

## Limitations

- **Trace JIT mcode is not yet AOT-compiled.** The produced binary
  runs the interpreter only; hot loops won't get the same native-speed
  treatment they would under `luna --jit foo.lua` with the runtime
  Cranelift JIT. The lowerer refactor that lets `cranelift-object`
  emit the same mcode at AOT time landed in Stage 3, but walking
  every reachable `Proto`'s hot loops + emitting them into the AOT
  binary's `.text` is the remaining work. **Post-v1.3.**
- **`loadstring(...)` at runtime works** — but it runs through the
  interpreter, no AOT codegen at runtime. Embedders that need
  runtime-`loadstring`-of-untrusted-source to be JIT-fast should use
  `luna-jit` instead.
- **MSVC link path is not driven** — Windows builds go through MinGW
  (`x86_64-pc-windows-gnu`). luna-aot doesn't shell out to `link.exe`
  directly; the MSVC arm of `link_aot_binary_for` returns a clear
  error directing users to either build from a Windows host or
  target windows-gnu.

## CLI surface

```text
luna-aot compile <input.lua> [--out <path>] [--target <triple>] [--dialect <5.X>] [--scaffold-only]
```

`--target` accepts any triple `TargetSpec::from_triple` parses
(tier 1: host + `*-apple-darwin`; tier 2: `*-unknown-linux-{gnu,musl}`,
`x86_64-pc-windows-gnu`). Unsupported arch / OS strings surface as a
clean error naming the supported set.

`--dialect` accepts `5.1` / `5.2` / `5.3` / `5.4` / `5.5` / `macrolua`;
default is `5.5`.

`--scaffold-only` falls back to the pre-Stage-4 path that emits a C
entry which only prints the embedded section length to stderr — useful
for benchmarking the link step in isolation or when the runtime
staticlib is known-broken on the host.

## Status — supply-chain delta

| crate | direct deps added |
|---|---|
| `luna-aot` (new) | `luna-core` (workspace) + `object 0.36` + `clap 4` (+ `tempfile` dev-only) |
| `luna-runtime-helpers` (new) | `luna-core` (workspace) — nothing else |
| `luna-core`      | **none** — 0-third-party-dep contract preserved |
| `luna-jit`       | **none** |
| `luna-jit-derive`| **none** |

CI `zero-dep` job continues to report luna-core's dep tree as one
row (itself).

The **deploy-side binary** is even cleaner: it only embeds
`luna-runtime-helpers` (which depends solely on `luna-core`) +
rust stdlib. Cranelift, `object`, `clap`, and `tempfile` are all
build-time-only for `luna-aot`; none of them ship in the produced
executable.
