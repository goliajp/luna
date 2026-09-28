# luna-aot

Ahead-of-time compiler from Lua source to a self-contained native
binary, built on `luna-core`'s VM. Sibling of `luna-core` (pure-interp
runtime, zero third-party deps) and `luna-jit` (the runtime
Cranelift JIT).

Lua source → luna bytecode dump → ELF/Mach-O/PE data section →
linked native binary that **constructs a `Vm` at process start,
undumps the bytecode, and runs it**. A warmup run under the JIT
recorder captures the hot traces, which are compiled to machine code
and linked into the same binary. Cross-compiling with `--target` and
Alpine (no Lua installed) deploys are supported; see
[`docs/aot.md`](../../docs/aot.md).

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

# windows x86_64 / MinGW from any unix host (needs mingw-w64):
sudo apt install gcc-mingw-w64-x86-64                             # debian/ubuntu
brew install mingw-w64                                            # macOS
luna-aot compile foo.lua --out foo.exe --target x86_64-pc-windows-gnu

# windows x86_64 / MSVC, native on Windows (Developer Command Prompt for VS 2022):
luna-aot compile foo.lua --out foo.exe --target x86_64-pc-windows-msvc
# or from a Unix host with the LLVM toolchain + Windows SDK mirror
# (clang-cl + lld-link via `brew install llvm` / `apt install clang lld`;
# Windows SDK + UCRT libs via an `xwin`-style setup):
luna-aot compile foo.lua --out foo.exe --target x86_64-pc-windows-msvc

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
| `x86_64-pc-windows-msvc` | rustup add | `clang-cl` + `lld-link` (LLVM) **or** `cl.exe` + `link.exe` (Visual Studio Build Tools 2022) | `brew install llvm` (provides both) | `apt install clang lld` (provides both) |

Anything missing surfaces as a concrete error message naming the
package; nothing is silently degraded.

## Why a separate crate

`luna-aot` pulls third-party deps (`object`, `clap` and
`cranelift`). Keeping it sibling
to `luna-jit` lets embedders pick exactly one of:

- **`luna-core`** — pure interp, zero third-party deps.
- **`luna-core + luna-jit`** — runtime JIT (mmap RWX, recorded traces).
- **`luna-core + luna-aot`** — offline compile to a deployable binary;
  the produced binary statically links `luna-core` + `luna-runtime-helpers`
  only (no `luna-jit`, no `luna-aot`, no Cranelift dynamic link).

The `luna-core` zero-third-party-dep contract is **unaffected** by
this crate.

## Pipeline

```text
foo.lua
  │ luna_core::frontend::parser::parse
  ▼
Chunk (AST)
  │ luna_core::compiler::compile_chunk
  ▼
Gc<Proto>
  │ warmup run → hot traces → Cranelift mcode (extra .o sections)
  │ luna_core::vm::dump::dump  (luna's own body format)
  ▼
Vec<u8>
  │ object::write::Object  (.luna.bytecode + bracket symbols, target-aware)
  ▼
foo.luna_bytecode.o   ELF / Mach-O / PE
  │ cc bytecode.o cmain.o libluna_runtime_helpers.a -o foo
  ▼
foo   single-binary, runs through luna_aot_run → Vm → call_value
```

`luna_aot_run` lives in `crates/luna-runtime-helpers/` as a dual
`staticlib + rlib`. The staticlib carries rust stdlib + all of
luna-core; the rlib is what `luna-aot`'s integration tests link
against so they can drive the same code path in-process without
shelling out to `cc`.

## Limitations

- **Trace coverage comes from one warmup run.** Only traces that fire
  during it are compiled to machine code; paths that turn hot later
  run in the embedded interpreter.
- **`loadstring(...)` at runtime works** — but it runs through the
  interpreter, no AOT codegen at runtime. Embedders that need
  runtime-`loadstring`-of-untrusted-source to be JIT-fast should use
  `luna-jit` instead.
- **MSVC link path requires Windows SDK libs on Unix hosts.** The
  `x86_64-pc-windows-msvc` target is fully driven (`clang-cl` /
  `cl.exe` for the C entry object, `lld-link` / `link.exe` for the
  final PE), but the final link needs Windows SDK + UCRT `.lib`
  files which are not installed by `brew install llvm` /
  `apt install lld` alone. On a Windows host (Developer Command
  Prompt for VS 2022) the toolchain is complete. On Unix hosts a
  `xwin`-style SDK mirror is required to make `lld-link` find
  `ucrt.lib` et al.; otherwise use `--target x86_64-pc-windows-gnu`
  (MinGW), which carries its own runtime via mingw-w64.

## CLI surface

```text
luna-aot compile <input.lua> [--out <path>] [--target <triple>] [--dialect <5.X>] [--scaffold-only]
```

`--target` accepts any triple `TargetSpec::from_triple` parses
(tier 1: host + `*-apple-darwin`; tier 2: `*-unknown-linux-{gnu,musl}`,
`x86_64-pc-windows-{gnu,msvc}`). Unsupported arch / OS strings
surface as a clean error naming the supported set.

`--dialect` accepts `5.1` / `5.2` / `5.3` / `5.4` / `5.5` / `macrolua`;
default is `5.5`.

`--scaffold-only` falls back to the pre-Stage-4 path that emits a C
entry which only prints the embedded section length to stderr — useful
for benchmarking the link step in isolation or when the runtime
staticlib is known-broken on the host.

## Status — supply-chain delta

| crate | direct deps added |
|---|---|
| `luna-aot` (new) | `luna-core` (workspace) + `object 0.40` + `clap 4` (+ `tempfile` dev-only) |
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
