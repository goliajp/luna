//! The Rust runtime stub that the AOT-produced binary calls at
//! process start. This module is **library code** today: the
//! scaffold's linked binary uses a tiny C `main` (see
//! [`crate::embed`]) that prints the bytecode length. The follow-up
//! session swaps that C entry out for this Rust stub so the binary
//! actually runs the embedded bytecode through a `Vm`.
//!
//! # Wiring (follow-up)
//!
//! Two paths to get this module's [`aot_main`] called from the
//! produced binary:
//!
//! 1. **Cargo bootstrap** (audit § Stage 6 Option A) — the embedder's
//!    AOT pipeline generates a tiny crate in a tempdir whose `main.rs`
//!    is:
//!    ```ignore
//!    fn main() -> std::process::ExitCode {
//!        std::process::ExitCode::from(luna_aot::runtime_stub::aot_main() as u8)
//!    }
//!    ```
//!    and whose `Cargo.toml` depends on `luna-aot = { path = "..." }`.
//!    Then `cargo build --release` produces the final binary; the
//!    bytecode object file emitted by [`crate::embed`] is linked in
//!    via a `build.rs` that calls `println!("cargo:rustc-link-arg=...")`.
//!
//! 2. **Pre-built staticlib** (audit § Stage 6 Option B, v1.4) — ship
//!    `libluna_core.a` per supported triple, link directly via `cc`,
//!    let this stub serve as the entry point through `#[no_mangle] pub
//!    extern "C" fn aot_main`.
//!
//! Either path requires the symbols [`crate::BYTECODE_START_SYMBOL`] /
//! [`crate::BYTECODE_END_SYMBOL`] to resolve at link time — both are
//! satisfied by the bytecode object [`crate::embed::embed_bytecode`]
//! writes.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

unsafe extern "C" {
    /// Linker-provided symbol marking the start of the embedded
    /// bytecode section. Resolves to the first byte of the dump bytes
    /// the AOT pipeline wrote into `.luna.bytecode`.
    #[link_name = "__luna_bytecode_start"]
    static BYTECODE_START: u8;

    /// Linker-provided symbol marking the byte after the last byte of
    /// the embedded bytecode. `&END - &START` is the section length.
    #[link_name = "__luna_bytecode_end"]
    static BYTECODE_END: u8;
}

/// Read the embedded bytecode section as a `&'static [u8]`.
///
/// # Safety
///
/// The bracket symbols must be defined by the linker, i.e. the
/// bytecode object file produced by [`crate::embed::embed_bytecode`]
/// (or an equivalent) is part of this binary's link set. Calling this
/// in a build that doesn't include the bytecode object is an
/// unresolved-symbol at link time, not a runtime failure.
pub fn embedded_bytecode() -> &'static [u8] {
    // SAFETY: the bracket symbols are linker-provided constants
    // pointing at the start and end of the `.luna.bytecode` section.
    // Building the slice from them is the standard
    // `__start_<sec>` / `__stop_<sec>` pattern (also used by linkers
    // for `__libc_*` and ELF `.init_array`).
    unsafe {
        let start = &BYTECODE_START as *const u8;
        let end = &BYTECODE_END as *const u8;
        let len = end.offset_from(start) as usize;
        std::slice::from_raw_parts(start, len)
    }
}

/// AOT-binary entry point. Constructs a fully-loaded `Vm`, undumps
/// the embedded bytecode into the heap via `Vm::load` (which routes
/// through `luna_core::vm::dump::undump` because the dump bytes start
/// with `\x1bLua`), calls the resulting closure with no args, and
/// returns the process exit code.
///
/// # Error handling
///
/// - Load failure (corrupted bytecode / dialect mismatch) prints the
///   error to `stderr` and exits 1.
/// - Runtime errors thrown by the script (uncaught `error(...)`) are
///   formatted via `Vm::error_text` + `Vm::take_error_traceback`,
///   printed to `stderr`, and the process exits 1.
/// - Successful runs exit 0 regardless of the script's return values
///   (matches PUC `lua` behaviour: scripts don't compose with
///   process exit codes unless they call `os.exit`).
pub fn aot_main() -> i32 {
    // The dialect baked into the produced binary defaults to 5.5.
    // The AOT pipeline always dumps with the dialect that compiled the
    // source, so a `Vm::new(Lua55)` matched against a 5.4 dump would
    // be a dialect mismatch — Phase ML's `MacroLua` falls back to the
    // 5.4 header, and `dump::undump` rejects mismatches with a clear
    // "header mismatch" message.
    //
    // Follow-up: the AOT pipeline emits a small `__luna_version` byte
    // alongside the bytecode bracket symbols so the stub picks the
    // matching dialect automatically. v1.3 scaffold pins 5.5 — the
    // CLI default — and rejects loads that don't match.
    let mut vm = Vm::new(LuaVersion::Lua55);

    // Enable bytecode loading explicitly. `Vm::new` defaults to
    // `bytecode_loading = true` (see `exec.rs:910`) but a future
    // sandbox-default flip would surface here; mark the intent so
    // the call site doesn't silently break.
    vm.set_bytecode_loading(true);

    let bytecode = embedded_bytecode();
    let closure = match vm.load(bytecode, b"=embedded") {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "luna-aot: load failed at line {}: {}",
                e.line,
                String::from_utf8_lossy(&e.msg)
            );
            return 1;
        }
    };

    match vm.call_value(Value::Closure(closure), &[]) {
        Ok(_results) => 0,
        Err(err) => {
            let msg = vm.error_text(&err);
            eprintln!("luna-aot: runtime error: {msg}");
            if let Some(tb) = vm.take_error_traceback() {
                eprintln!("{tb}");
            }
            1
        }
    }
}
