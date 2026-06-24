#![warn(missing_docs)]
//! luna-runtime-helpers — the static-link runtime entry for the
//! binaries that `luna-aot` produces.
//!
//! # Role in the v1.3 Phase AOT pipeline
//!
//! `luna-aot compile foo.lua --out foo` walks:
//!
//! 1. Parse + compile `foo.lua` to a luna bytecode dump (Stages 1-2).
//! 2. Emit a `.luna.bytecode` data section in a fresh `.o` (Stage 5).
//! 3. **Build this crate as a `staticlib`** — `libluna_runtime_helpers.a`
//!    bundles the rust stdlib + luna-core + this thin C-ABI entry.
//! 4. Emit a tiny C `main` that calls into [`luna_aot_run`] passing
//!    the bracket-symbol bounds of the bytecode section (Stage 6).
//! 5. `cc` links `bytecode.o` + `main.o` + `libluna_runtime_helpers.a`
//!    + `-lpthread -ldl -lm` into the final executable.
//!
//! The produced binary at run time:
//!
//! - the C `main` calls [`luna_aot_run(bytecode_ptr, len)`][luna_aot_run]
//! - [`luna_aot_run`] constructs a `Vm`, allows bytecode loading,
//!   calls `Vm::load(slice, b"=embedded")` (which routes through
//!   `luna_core::vm::dump::undump` because the slice starts with
//!   `\x1bLua`), then `Vm::call_value` on the resulting root closure
//! - normal `print(...)` from the script lands on stdout via
//!   `std::io::stdout` inside luna-core's builtins (no surprises)
//! - exit code 0 on success, 1 on load / runtime error
//!
//! # Why a separate crate (not folded into `luna-aot`)
//!
//! `luna-aot` is the **build-time** tool — it pulls `object` + `clap`
//! and eventually all of cranelift. The **deploy-side** binary must
//! not pull cranelift; it only needs the luna interp. Splitting this
//! entry into its own crate keeps the deploy-side `.a` tight (rust
//! stdlib + luna-core only) and lets `luna-aot` invoke
//! `cargo build -p luna-runtime-helpers --release` without dragging
//! its own dep tree into the link.
//!
//! # luna-core 0-third-party-dep contract
//!
//! Unchanged. `cargo tree -p luna-core --prefix none | grep -cE " v[0-9]"`
//! continues to report 1. This crate sits **above** luna-core in the
//! dep graph; nothing here flows back into luna-core.

use std::panic;
use std::slice;

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// AOT-binary C-ABI entry. The auto-generated C `main` calls this
/// once with a pointer + length pair derived from the bracket
/// symbols `__luna_bytecode_start` / `__luna_bytecode_end` that
/// `luna-aot` emits into the `.luna.bytecode` section.
///
/// Returns the process exit code:
///
/// - `0` — script ran to completion (any `return` values are ignored,
///   matching `lua foo.lua` semantics: PUC discards top-level returns)
/// - `1` — bytecode load failed (header mismatch, truncated dump,
///   unsupported opcode), runtime error, or a panic escaped luna-core
///
/// # Safety
///
/// `bytecode` must point at `len` bytes of a valid luna bytecode dump
/// (the bytes that `luna_core::vm::dump::dump` produces). The slice
/// must remain live and unmutated for the duration of the call —
/// in the AOT-binary use case the bytes live in the read-only data
/// segment of the binary itself, so this is trivially satisfied.
///
/// `len` must not be 0 (an empty dump is rejected by `Vm::load` with
/// a clear error, but we early-out before constructing the slice to
/// avoid a `from_raw_parts(null, 0)` UB corner). `len == 0` returns 1.
///
/// Panics inside luna-core (which would normally tear down a Rust
/// host process) are caught here and turned into exit code 1 with the
/// payload printed to stderr.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_aot_run(bytecode: *const u8, len: usize) -> i32 {
    // Defensive: a null/zero-len section means the linker didn't wire
    // the bytecode object — clearer error than a slice deref.
    if bytecode.is_null() || len == 0 {
        eprintln!(
            "luna-runtime-helpers: embedded bytecode section is empty \
             (ptr={bytecode:p}, len={len}) — was the bytecode .o linked in?"
        );
        return 1;
    }

    // SAFETY: caller contract — `bytecode` points at `len` valid bytes
    // for the duration of this call. In the AOT-binary deploy shape
    // these bytes live in the binary's `.rodata` and are immutable
    // for the lifetime of the process.
    let bytecode_slice: &'static [u8] = unsafe { slice::from_raw_parts(bytecode, len) };

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| run_inner(bytecode_slice)));
    match result {
        Ok(code) => code,
        Err(payload) => {
            // Mirror the std panic hook's payload-shape extraction so
            // users see roughly the same message a panic would print
            // when not caught.
            let msg = panic_payload_text(&payload);
            eprintln!("luna-runtime-helpers: vm panicked: {msg}");
            1
        }
    }
}

/// The Rust-side body of [`luna_aot_run`]. Split out so the C-ABI
/// boundary stays minimal and the `panic::catch_unwind` closure has
/// a clear, self-contained body.
fn run_inner(bytecode: &[u8]) -> i32 {
    // The dialect picked here governs which header bytes `Vm::load`
    // accepts. v1.3 floor pins this to 5.5 — the `luna-aot` CLI
    // default. A `--dialect 5.4` invocation would compile against
    // 5.4's header; the v1.3 floor relies on the embedder running the
    // AOT pipeline with a matching dialect on both sides. Stage 5+
    // can embed a `__luna_version` byte and dispatch dynamically.
    let mut vm = Vm::new(LuaVersion::Lua55);

    // `Vm::new` defaults to `bytecode_loading = true` (see luna-core
    // `exec.rs:910`), but a future sandbox-default flip would break
    // silently here. Setting it explicitly makes the intent legible
    // and survives any default change.
    vm.set_bytecode_loading(true);

    let closure = match vm.load(bytecode, b"=embedded") {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "luna-runtime-helpers: load failed at line {}: {}",
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
            eprintln!("luna-runtime-helpers: runtime error: {msg}");
            if let Some(tb) = vm.take_error_traceback() {
                eprintln!("{tb}");
            }
            1
        }
    }
}

/// Best-effort extraction of a panic payload's display text. Matches
/// the rust stdlib's payload-shape handling so users see the same
/// "panicked at … : <msg>" snippet shape they would expect.
fn panic_payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-string panic payload)".to_string()
    }
}

/// Convenience entry for in-process Rust drivers (`luna-aot`'s
/// integration tests, embedders that want to invoke the same code
/// path without going through `cc` link).
///
/// Identical semantics to [`luna_aot_run`] but skips the raw-ptr +
/// `catch_unwind` shim. Panics propagate.
pub fn run_bytecode(bytecode: &[u8]) -> i32 {
    run_inner(bytecode)
}

/// Force-link the C-ABI symbol so a `cargo build` of a dependent
/// rlib doesn't dead-strip it. Without this, the symbol is technically
/// reachable (no_mangle + extern "C"), but rustc / lld can be over-
/// eager in some pipelines; calling this from `lib.rs::pre_main` or
/// from a `build.rs` artifact ensures the staticlib export survives.
///
/// This is a `pub fn` so dependent test binaries that build against
/// the `rlib` crate-type pull the symbol via the live reference here.
/// The staticlib `crate-type` path doesn't need it (staticlib emit
/// preserves no_mangle externs by construction), but the dual-crate-
/// type setup gives us both for free.
pub const fn force_link_aot_entry() -> unsafe extern "C" fn(*const u8, usize) -> i32 {
    luna_aot_run
}
