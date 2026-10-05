# Ahead-of-Time compilation (`luna-aot`)

`luna-aot` is a build-time tool that turns a Lua source file into a
self-contained native binary. The output ships without `luna` itself
installed on the deployment host; the embedded runtime is statically
linked.

This document covers:

- when to reach for AOT vs JIT vs plain interp
- the one-command happy path
- cross-compile workflow
- what the produced binary actually contains (size + section
  breakdown)
- known limitations
- how to inspect what landed

For the threat model that frames AOT-binary trust, see
[`security.md`](security.md). For per-crate disk + binary size
budgets and reduction levers, see [`binary-size.md`](binary-size.md).

---

## 1. When to use AOT

| Goal | Use |
|---|---|
| One redistributable binary, no `luna` on host | **`luna-aot`** |
| Live-coding REPL, dev-time iteration | `luna` (JIT) |
| `cargo add luna-core` library embed (no JIT, no AOT) | `luna-core` |
| `cargo add luna-jit` library embed (JIT) | `luna-jit` |

AOT is appropriate when:

- The Lua source is **frozen at build time** (no eval of user-supplied
  scripts on the deploy host).
- You want **single-binary deploy** (no `liblua*`, no `liblua-jit*`
  dynamic loading, no JIT codegen at startup).
- You're willing to trade JIT's peak throughput on long-running
  loops for predictable startup latency + smaller deploy surface.

If the deploy host can spare ~12 MB of binary and you want long-loop
performance, prefer **`luna-jit`** statically linked into a Rust
binary that drives `Vm::eval` — JIT mcode beats AOT on hot loops
because the AOT mcode is captured from a single warmup run rather
than the workload's steady-state shapes.

---

## 2. One-command happy path

```sh
cargo install luna-aot   # one-time
luna-aot compile hello.lua --out hello
./hello                  # standalone native binary
```

That's it. The default dialect is **Lua 5.5**; pass `--dialect 5.4`
(also accepts `5.1`, `5.2`, `5.3`, `macrolua`) for older PUC
compatibility — the source must match the dialect's syntax + library
surface.

Behind the scenes, `luna-aot compile`:

1. Parses + compiles the Lua source via the chosen dialect's frontend
   (`crates/luna-core/src/frontend/`).
2. Emits a luna bytecode dump (the same format `luna`'s REPL writes
   internally) into a data section of the produced object file.
3. Runs a warmup pass under the JIT recorder to capture hot traces
   that survive an entire eval; those traces are lowered to native
   mcode and emitted as additional `.o` sections.
4. Links `luna-runtime-helpers` (the static-link runtime entry
   crate) against the generated `.o` files; the entry symbol
   `luna_aot_run_dialect(ptr, len, dialect) -> i32` runs the embedded
   bytecode on a `Vm` of the dialect it was compiled for (the generated
   C `main` passes it). `luna_aot_run(ptr, len)` is the same entry for
   a 5.5 dump.
5. Produces the final native binary at the path passed via `--out`
   (or the input stem if `--out` is omitted).

---

## 3. Cross-compile

```sh
# from a macOS aarch64 host, target Linux x86_64
luna-aot compile foo.lua --target x86_64-unknown-linux-gnu --out foo.linux

# Windows MinGW PE
luna-aot compile foo.lua --target x86_64-pc-windows-gnu --out foo.exe
```

The `--target` flag accepts the standard `target_lexicon::Triple`
syntax (`<arch>-<vendor>-<sys>-<env>`). For trace-mcode codegen the
Cranelift `all-arch` backend feature is enabled in `luna-aot`'s
crate, so the **build-time** binary supports every Cranelift target
without rebuilding `luna-aot` itself. The **link** step uses the
system `cc`, a named cross compiler, or for Windows MSVC the tools
listed below; a missing tool is an error that names what to install.

Tier-1 verified on macOS aarch64 host: `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
`x86_64-unknown-linux-musl`, `x86_64-pc-windows-gnu`,
`x86_64-pc-windows-msvc`.

### Windows

A Windows host is a first-class AOT host: CI builds and runs AOT
binaries on `windows-latest` for both Windows targets, traces
included.

| Target | Host | C compiler + linker | Setup |
|---|---|---|---|
| `x86_64-pc-windows-msvc` (default on a Windows host with the MSVC Rust toolchain) | Windows | `cl.exe` + `link.exe` | Visual Studio 2017 or newer, or its Build Tools, with the "Desktop development with C++" workload. luna-aot finds the newest install and sets `INCLUDE` / `LIB` / `PATH` itself, so a plain `cmd` or PowerShell works; inside a Developer Command Prompt its environment is used as is |
| `x86_64-pc-windows-msvc` | Windows, no Visual Studio C++ tools | `clang-cl` + `lld-link` from LLVM on `PATH` | The MSVC CRT and Windows SDK libraries from `xwin splat`, found as described below. CI runs the AOT tests this way on `windows-latest` with Visual Studio hidden (vswhere, luna-aot and lld-link find no installation) |
| `x86_64-pc-windows-msvc` | macOS / Linux | `clang-cl` + `lld-link` (and `llvm-lib`) | `brew install llvm lld` / `apt install clang lld llvm`, `rustup target add x86_64-pc-windows-msvc`, and the libraries from `xwin splat`, found as described below. CI builds on Linux and runs the binaries under Wine, and builds on macOS and runs them on Windows |
| `x86_64-pc-windows-gnu` | Windows | MinGW `x86_64-w64-mingw32-gcc`, or its plain `gcc` | `rustup target add x86_64-pc-windows-gnu` and a MinGW-w64 toolchain on `PATH` (MSYS2, or the one on GitHub's runners) |
| `x86_64-pc-windows-gnu` | macOS / Linux | `x86_64-w64-mingw32-gcc` | `brew install mingw-w64` / `apt install gcc-mingw-w64-x86-64` |

#### The MSVC libraries without Visual Studio

Without Visual Studio, the MSVC target needs the MSVC C runtime and the
Windows SDK from somewhere. [`xwin`](https://github.com/Jake-Shadle/xwin)
downloads them from Microsoft's servers (accepting Microsoft's license)
and writes them into one directory:

```sh
xwin --accept-license splat --output ~/xwin
LUNA_AOT_MSVC_SYSROOT=~/xwin luna-aot compile foo.lua --target x86_64-pc-windows-msvc --out foo.exe
```

luna-aot picks the MSVC tools in this order, the way it looks for Visual
Studio on a Windows host:

1. `CC` / `LD`, used as given.
2. `LUNA_AOT_MSVC_SYSROOT` set: `clang-cl` and `lld-link` from `PATH`,
   with the headers and libraries of that directory. Visual Studio is not
   looked for, so this also forces LLVM's tools on a Windows host that
   has Visual Studio. The directory may hold either `xwin splat` layout,
   the default one (`crt/`, `sdk/`) or `--use-winsysroot-style`
   (`VC/Tools/MSVC`, `Windows Kits/10`); a directory with neither is an
   error.
3. On a Windows host, `cl.exe` and `link.exe` from Visual Studio.
4. `clang-cl` and `lld-link` from `PATH`, with the directory
   [`cargo xwin`](https://github.com/rust-cross/cargo-xwin) keeps in its
   cache when it exists (`~/.cache/cargo-xwin/xwin` on Linux,
   `~/Library/Caches/cargo-xwin/xwin` on macOS,
   `%LOCALAPPDATA%\cargo-xwin\xwin` on Windows), so a machine that has
   built with `cargo xwin` needs no setting. Without such a directory
   `lld-link` reads `LIB`, and on Windows finds an installed SDK itself.

When the tools are LLVM's and a sysroot was found, the cargo build of
the runtime staticlib gets the same setup: `CC_<target>=clang-cl` and the
sysroot's headers in `CFLAGS_<target>` for the C part of `luna-jit`, and
`CARGO_TARGET_<TARGET>_LINKER=lld-link` with the sysroot's library
directories for the `luna-jit` DLL that cargo links on the way. A
variable you set yourself is left alone. These variables split on
spaces, so the sysroot path must not contain any.

`CC` and `LD` override the compiler and linker for either target. On a
Windows target an `--out` path without an extension gets `.exe`.

The two targets differ in what the binary needs at run time. Both use
the Universal CRT that ships with Windows 10 and later (the
`api-ms-win-crt-*` imports). The MSVC binary also imports
`VCRUNTIME140.dll`, which comes with the Visual C++ Redistributable;
install it, or ship it next to the binary, on a machine without Visual
Studio. The MinGW binary built by a UCRT MinGW toolchain (MSYS2's
`ucrt64`, GitHub's runners) needs nothing beyond Windows; one built by
an older `msvcrt` toolchain imports `msvcrt.dll` instead. AOT traces,
the per-dialect `Vm` and the `.lt_*` sections the runtime reads behave
the same on both.

Windows on ARM (`aarch64-pc-windows-msvc`) is not supported: Cranelift's
PE/COFF writer lacks a relocation the AArch64 traces need.

Tier-2 (build-succeeds but actual-run not CI-verified on every
release): `aarch64-unknown-linux-musl`, RISC-V, s390x.

For musl/Alpine deployment specifically, install `musl-cross` via
`brew install musl-cross` on the build host before invoking
`--target x86_64-unknown-linux-musl`; the link step needs the matching
`musl-gcc`.

---

## 4. What the binary contains

A `production_like.lua` chunk (~1.5k LOC) compiled with default
release profile + `strip` produces ~4.5 MiB on macOS aarch64
(measured 2026-06-25).

Section breakdown (`size -m`, release-stripped):

| Section | Bytes | Notes |
|---|---:|---|
| `__TEXT/__text` | 3.4 MiB | Static interpreter + GC + stdlib + Cranelift-emitted trace mcode |
| `__TEXT/__cstring` | ~250 KiB | Symbol strings + diagnostic message literals |
| `__TEXT/__eh_frame` + `__unwind_info` | 413 KiB | Unwind tables — `panic="abort"` profile can shrink this |
| `.luna.bytecode` | 96 KiB | The embedded chunk's bytecode dump (proportional to source size) |
| `__DATA/__data` | ~150 KiB | Per-trace AOT metadata tables + global statics |
| `__LINKEDIT` | 272 KiB | Symbol table + load commands (post-strip) |

For smaller workloads, the **runtime floor** dominates: a 1-liner
`hello.lua` compiles to ~4.5 MiB stripped — only ~82 KiB smaller
than the 1.5k LOC `production_like.lua`. Subsequent embedded-source
growth is roughly linear with the source's emitted bytecode (~1
byte of `.luna.bytecode` per Lua opcode plus constants).

---

## 5. Known limitations

- **Trace-mcode coverage is single-warmup-pass**. The AOT pipeline
  runs the source under the trace recorder once; only the hot loops
  that fire during that warmup are captured as native mcode. Cold
  paths that fire later run through the embedded interpreter (still
  fast — luna's interp is competitive in its own right) without the
  trace-JIT speedup. If a workload has multiple hot-loop shapes
  that don't all surface in a single warmup, JIT (live `luna`) will
  outperform AOT on the un-captured shapes.
- **Inline-side-exit chain reloc** (`per_exit_inline`) is wired
  in the AOT format but its trigger pattern is recorder-heuristic
  dependent — some self-recursive shapes don't currently emit
  inline-chain dispatches in the warmup pass.
- **No runtime code patching**. The produced binary cannot install
  new traces post-deploy. For workloads that change shape over
  weeks, prefer `luna-jit` + a long-running `Vm`.
- **No incremental compile**. Each `luna-aot compile` invocation
  re-runs the warmup pass; for tight build/test cycles, use the
  JIT path during dev and AOT only for ship binaries.
- **`luna-aot` is a build-time tool, not a runtime dep**. The
  produced binary depends on `luna-runtime-helpers` (statically
  linked); `luna-aot` itself is not on the runtime crate graph.

---

## 6. Inspecting what landed

The produced binary is a stock native executable. Standard tools
work:

```sh
# macOS section breakdown
size -m ./hello

# Linux ELF section + symbol dump
readelf -S ./hello | grep luna
nm ./hello | grep luna_aot

# AOT-specific section walk (luna_trace_meta is in __DATA)
otool -l ./hello | grep -A4 luna_trace_meta
```

For Windows PE, the same sections live under shorter names — `.lt_meta`
holds the AOT trace metadata, `.lt_skix` the string-key index. The
linker walks these at runtime to install the trace mcode against
the embedded Vm.

The standard tools above plus `size -m` cover most deployment-time
inspection needs.

---

## 7. See also

- [`embedding.md`](embedding.md) — `luna-core` / `luna-jit` library
  embed cookbook (the non-AOT path)
- [`binary-size.md`](binary-size.md) — per-crate + AOT output disk
  budgets + reduction levers (`panic="abort"`, LZ4 bytecode
  compress, cranelift `all-arch` opt-out)
- [`security.md`](security.md) — threat model for AOT binaries
  (the embedded bytecode is trusted; the embedded Vm enforces
  the same sandbox boundaries as the library path)
- [`compatibility.md`](compatibility.md) — per-dialect feature
  matrix (`--dialect` arg)
- [`migration-v1-to-v2.md`](migration-v1-to-v2.md) — AOT-binary
  breaking changes between major versions
