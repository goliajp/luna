#![warn(missing_docs)]
//! luna-runtime-helpers — the static-link runtime entry for the
//! binaries that `luna-aot` produces.
//!
//! # Role in the AOT pipeline
//!
//! `luna-aot compile foo.lua --out foo` walks:
//!
//! 1. Parse + compile `foo.lua` to a luna bytecode dump.
//! 2. Emit a `.luna.bytecode` data section in a fresh `.o`.
//! 3. **Build this crate as a `staticlib`** — `libluna_runtime_helpers.a`
//!    bundles the rust stdlib + luna-core + this thin C-ABI entry.
//! 4. Emit a tiny C `main` that calls into [`luna_aot_run_dialect`]
//!    passing the bracket-symbol bounds of the bytecode section and the
//!    dialect the script was compiled for.
//! 5. `cc` links `bytecode.o` + `main.o` + `libluna_runtime_helpers.a`
//!    + `-lpthread -ldl -lm` into the final executable.
//!
//! The produced binary at run time:
//!
//! - the C `main` calls
//!   [`luna_aot_run_dialect(bytecode_ptr, len, dialect)`][luna_aot_run_dialect]
//! - [`luna_aot_run_dialect`] constructs a `Vm` of that dialect, allows bytecode loading,
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

// Windows PE/COFF section walker.
// Used by `aot_strkey_resolver` and `aot_trace_registry` to enumerate
// the deploy-side `lt_meta` / `lt_skix` sections on Windows, where
// the Unix-style `__start_/__stop_` bracket symbol convention isn't
// synthesized by `link.exe` / `lld-link`. Hand-rolled winapi externs
// keep the dep story unchanged (no `windows-sys` / `winapi` crate
// added). See module docs for the design rationale.
#[cfg(all(target_os = "windows", feature = "jit-helpers"))]
mod windows_section;

/// The `dialect` code [`luna_aot_run_dialect`] takes for `version`: the
/// version number without its dot (51 for Lua 5.1, ..., 55 for 5.5), and
/// 254 for MacroLua. `luna-aot` writes this number into the C `main` it
/// generates.
pub const fn dialect_code(version: LuaVersion) -> u32 {
    match version {
        LuaVersion::Lua51 => 51,
        LuaVersion::Lua52 => 52,
        LuaVersion::Lua53 => 53,
        LuaVersion::Lua54 => 54,
        LuaVersion::Lua55 => 55,
        LuaVersion::MacroLua => 254,
    }
}

/// The dialect a [`dialect_code`] stands for; `None` for any other number.
pub const fn dialect_from_code(code: u32) -> Option<LuaVersion> {
    match code {
        51 => Some(LuaVersion::Lua51),
        52 => Some(LuaVersion::Lua52),
        53 => Some(LuaVersion::Lua53),
        54 => Some(LuaVersion::Lua54),
        55 => Some(LuaVersion::Lua55),
        254 => Some(LuaVersion::MacroLua),
        _ => None,
    }
}

/// [`luna_aot_run_dialect`] for a Lua 5.5 dump. A binary that `luna-aot`
/// links calls [`luna_aot_run_dialect`] with the dialect it compiled the
/// script for; this entry stays for C hosts written against it.
///
/// # Safety
///
/// Same contract as [`luna_aot_run_dialect`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn luna_aot_run(bytecode: *const u8, len: usize) -> i32 {
    // SAFETY: the caller upholds this function's contract, which is
    // `luna_aot_run_dialect`'s for the same two arguments
    unsafe { luna_aot_run_dialect(bytecode, len, dialect_code(LuaVersion::Lua55)) }
}

/// AOT-binary C-ABI entry. The auto-generated C `main` calls this
/// once with a pointer + length pair derived from the bracket
/// symbols `__luna_bytecode_start` / `__luna_bytecode_end` that
/// `luna-aot` emits into the `.luna.bytecode` section, and the
/// [`dialect_code`] of the dialect the script was compiled for. The
/// `Vm` that runs the dump is created for that dialect: luna's dump
/// header does not tell 5.1 / 5.2 / 5.5 apart, and a 5.5 `Vm` refuses
/// a 5.3 or 5.4 dump as foreign PUC bytecode.
///
/// Returns the process exit code:
///
/// - `0` — script ran to completion (any `return` values are ignored,
///   matching `lua foo.lua` semantics: PUC discards top-level returns)
/// - `1` — unknown `dialect`, bytecode load failed (header mismatch,
///   truncated dump, unsupported opcode), runtime error, or a panic
///   escaped luna-core
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
pub unsafe extern "C" fn luna_aot_run_dialect(
    bytecode: *const u8,
    len: usize,
    dialect: u32,
) -> i32 {
    let Some(version) = dialect_from_code(dialect) else {
        eprintln!("luna-runtime-helpers: unknown dialect code {dialect}");
        return 1;
    };
    // Defensive: a null/zero-len section means the linker didn't wire
    // the bytecode object — clearer error than a slice deref.
    if bytecode.is_null() || len == 0 {
        eprintln!(
            "luna-runtime-helpers: embedded bytecode section is empty \
             (ptr={bytecode:p}, len={len}) — was the bytecode .o linked in?"
        );
        return 1;
    }

    // Pin the `luna_jit_*` helper symbols
    // into the staticlib's link graph by way of a runtime call edge
    // from this entry. Without a call edge, fat-LTO observes that
    // `force_link_jit_helpers` is unreferenced from the staticlib's
    // exported API surface and elides the entire pin module — which
    // cascades and lets the staticlib bundling step drop every
    // `luna_jit_*`-defining cgu from `luna-jit`'s rlib. The result
    // would be a clean `cargo build` followed by an unresolved-symbol
    // failure at the AOT binary's link step ("undefined reference to
    // `_luna_jit_table_get_field`"). `black_box` on the return value
    // is what makes LTO unable to fold the call to a no-op.
    #[cfg(feature = "jit-helpers")]
    {
        let n = jit_helpers_pin::force_link_jit_helpers();
        std::hint::black_box(n);
    }

    // SAFETY: `bytecode` is non-null (checked above) and points at `len`
    // bytes that stay readable and unchanged for this call (# Safety);
    // the slice is only used by `run_inner`, which returns before this
    // call does
    let bytecode_slice: &'static [u8] = unsafe { slice::from_raw_parts(bytecode, len) };

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        run_inner(bytecode_slice, version)
    }));
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
fn run_inner(bytecode: &[u8], version: LuaVersion) -> i32 {
    // `Vm::load` takes a luna dump only from a `Vm` of the dialect that
    // wrote it; the library set and the number semantics are the
    // dialect's too.
    let mut vm = Vm::new(version);

    // `Vm::new` defaults to `bytecode_loading = true` (see luna-core
    // `exec.rs:910`), but a future sandbox-default flip would break
    // silently here. Setting it explicitly makes the intent legible
    // and survives any default change.
    vm.set_bytecode_loading(true);

    // Install the
    // real Cranelift JIT backend (= `enter_jit` that pins `JIT_VM` /
    // `JIT_CL` TLS) BEFORE any AOT-emitted trace mcode dispatches.
    //
    // Without this swap, the deploy `Vm` runs `NullJitBackend.enter`,
    // which is a no-op — `JIT_VM` TLS stays null, and the first AOT
    // trace that calls any `luna_jit_*` helper (e.g. `_table_get_field`,
    // `_op_get_tab_up`, `_table_set_int`) hits `debug_assert!(!p.is_null
    // ())` in debug builds or dereferences null in release →
    // SIGSEGV.
    //
    // Recorder is irrelevant on the deploy side (AOT traces install
    // before any record fires; the active `trace_compiler` would never
    // get called), but `IntChunkCompiler::enter` IS load-bearing —
    // it's the function the dispatcher calls right before
    // `entry_fn(reg_state)`.
    //
    // `install_jit_backend` is luna-core API; `CraneliftBackend`
    // implements both `IntChunkCompiler` (whose `enter` is what we
    // actually need) and `TraceCompiler`. Wrap behind `jit-helpers`
    // feature so a future no-JIT-on-deploy build can opt out (in
    // which case AOT traces that touch helpers would have to be
    // filtered at AOT-compile time — currently all of them do).
    #[cfg(feature = "jit-helpers")]
    {
        vm.install_jit_backend(
            luna_jit::jit_backend::CraneliftBackend,
            luna_jit::jit_backend::CraneliftBackend,
        );
        // NOTE: `trace_enabled = true` (the default) is
        // load-bearing for AOT dispatch too — `Vm::run`'s trace
        // lookup gate is `if self.jit.trace_enabled`, used for BOTH
        // runtime-compiled traces AND AOT-installed traces.
        // Disabling here would silently skip the AOT install's
        // dispatch. Runtime re-recording for back-edges the AOT
        // didn't cover is fine — same pattern interp + JIT uses.
    }

    // Interned-string slot resolver. Runs BEFORE `vm.load` so the resulting closure's
    // first dispatch into AOT mcode sees populated slots. Idempotent
    // and tolerates the empty-section case (binary linked zero AOT
    // traces): both bracket symbols collapse to the same address, the
    // walk terminates immediately.
    //
    // `vm.load` interns its own strings into `vm.heap`'s string table,
    // which the resolver also populates here; intern is idempotent, so
    // an AOT-time and load-time intern of the same UTF-8 bytes
    // resolve to the same `Gc<LuaStr>` pointer — the load-bearing
    // invariant that lets trace mcode pass interned-key pointers to
    // the `luna_jit_*_field` helpers.
    #[cfg(feature = "jit-helpers")]
    {
        let resolved = aot_strkey_resolver::resolve_all(&mut vm);
        if std::env::var_os("LUNA_AOT_PROBE").is_some() {
            eprintln!("luna-runtime-helpers: aot_strkey_resolved = {resolved}");
        }
        // Inline chain slot population. Must run BEFORE `aot_trace_registry::install_all`
        // so the dispatcher's first AOT-mcode dispatch finds populated
        // chain slots (the IR's `luna_jit_trace_materialize_frames(n,
        // ptr)` call would otherwise deref NULL). No Vm interaction
        // needed — the chains are pure metadata, owned by leaked Rcs.
        let chains_resolved = aot_inline_chain_resolver::resolve_all();
        if std::env::var_os("LUNA_AOT_PROBE").is_some() {
            eprintln!("luna-runtime-helpers: aot_inline_chains_resolved = {chains_resolved}");
        }
    }

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

    // Install AOT-emitted traces
    // against the loaded chunk's proto tree. Runs after `vm.load`
    // (the resolver needs the closure's proto as the BFS root) and
    // BEFORE `vm.call_value` (so the dispatcher's first back-edge
    // visit finds the installed trace and fires AOT mcode, instead
    // of bumping `trace_hot_count` from zero and going through the
    // runtime recorder again). Empty-section tolerant: a binary with
    // zero linked AOT trace `.o`s sees `installed == 0`, fall through
    // to runtime JIT.
    #[cfg(feature = "jit-helpers")]
    {
        let root_proto = closure.proto;
        // before any trace runs: an inlined call checks its callee
        // against these slots
        let protos = aot_proto_resolver::resolve_all(&vm, root_proto);
        if std::env::var_os("LUNA_AOT_PROBE").is_some() {
            eprintln!("luna-runtime-helpers: aot_proto_slots_resolved = {protos}");
        }
        let installed = aot_trace_registry::install_all(&mut vm, root_proto);
        if std::env::var_os("LUNA_AOT_PROBE").is_some() {
            eprintln!("luna-runtime-helpers: aot_trace_install_count = {installed}");
        }
    }

    let rc = match vm.call_value(Value::Closure(closure), &[]) {
        Ok(_results) => 0,
        Err(err) => {
            let msg = vm.error_text(&err);
            eprintln!("luna-runtime-helpers: runtime error: {msg}");
            if let Some(tb) = vm.take_error_traceback() {
                eprintln!("{tb}");
            }
            1
        }
    };

    // Post-run probe for the inline-chain reloc fire path. Counts
    // every entry to `luna_jit_trace_materialize_frames` from trace
    // mcode (JIT-baked OR AOT slot-loaded). In an AOT-only binary
    // any non-zero value is direct evidence that the chain
    // reloc path actually fires at runtime — the resolver-side probe
    // (`aot_inline_chains_resolved`) only confirms the slot got
    // populated, not that any AOT mcode dispatch ever loaded it.
    #[cfg(feature = "jit-helpers")]
    if std::env::var_os("LUNA_AOT_PROBE").is_some() {
        let fires = luna_jit::jit_backend::trace_materialize_frames_fires();
        eprintln!("luna-runtime-helpers: trace_materialize_frames_fires = {fires}");
    }

    rc
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
/// path without going through `cc` link), for a Lua 5.5 dump.
///
/// Identical semantics to [`luna_aot_run`] but skips the raw-ptr +
/// `catch_unwind` shim. Panics propagate.
pub fn run_bytecode(bytecode: &[u8]) -> i32 {
    run_inner(bytecode, LuaVersion::Lua55)
}

/// [`run_bytecode`] for a dump of any dialect: identical semantics to
/// [`luna_aot_run_dialect`] without the raw-ptr + `catch_unwind` shim.
pub fn run_bytecode_as(bytecode: &[u8], version: LuaVersion) -> i32 {
    run_inner(bytecode, version)
}

// Re-export of the 46 `luna_jit_*` Cranelift
// trace-mcode helpers from `luna-jit::jit_backend`. AOT binaries whose
// embedded `.o` calls these helpers (any trace that does table get/set,
// upvalue read, concat, etc.) needs them resolvable as strong externs
// at static-link time.
//
// The challenge: `luna-runtime-helpers` does not call these symbols
// itself, so a plain `pub use luna_jit::jit_backend::luna_jit_*` would
// be dead-stripped by `rustc`'s rlib → staticlib bundling step (Rust's
// `staticlib` crate-type only preserves transitive `#[no_mangle]`
// symbols that are reachable via a `pub` re-export chain whose roots
// are themselves marked `#[used]` or referenced from a kept root).
//
// Strategy: a single `#[used] static` whose contents is an array of
// raw fn pointers — one per helper. The static is itself reachable via
// a `pub` from `lib.rs` (`force_link_jit_helpers`), which gives the
// `staticlib` linker a strong reason to keep the array's contents,
// which in turn pins each helper's `#[no_mangle] pub unsafe extern "C"`
// definition through the rlib graph. The array is never *read* at run
// time; it's a link-time anchor only.
//
// Verified post-build:
//   `nm target/release/libluna_runtime_helpers.a | grep " T _luna_jit_" | wc -l`
//   reports 46 (one per helper).
// Re-export the helpers at the crate root. This pulls them into our
// `pub` surface so rustc treats them as kept symbols. The
// `extern "C"` + `#[no_mangle]` on the upstream definitions means
// the linker sees them under their bare names (`luna_jit_*`) — the
// `pub use` doesn't introduce a mangled wrapper. Combined with the
// runtime call edge from `luna_aot_run` → `force_link_jit_helpers`
// → helper calls (see `jit_helpers_pin` below), the staticlib
// bundling step is forced to pull in the defining cgus.
#[cfg(feature = "jit-helpers")]
pub use luna_jit::jit_backend::{
    luna_jit_fmod, luna_jit_head_closure, luna_jit_materialize_sunk_table,
    luna_jit_math_fn_is_library, luna_jit_new_table, luna_jit_new_table_sized, luna_jit_op_close,
    luna_jit_op_closure, luna_jit_op_concat, luna_jit_op_get_tab_up,
    luna_jit_op_get_tab_up_checked, luna_jit_op_self_checked, luna_jit_op_tforcall,
    luna_jit_park_deopt, luna_jit_self_upval_check, luna_jit_spill_to_stack, luna_jit_stack_load,
    luna_jit_stack_tag, luna_jit_stack_update_raw, luna_jit_str_buf_acquire,
    luna_jit_str_buf_extend, luna_jit_str_buf_intern, luna_jit_str_buf_release, luna_jit_str_sub,
    luna_jit_suppress_trace_admit, luna_jit_table_get_field, luna_jit_table_get_field_checked,
    luna_jit_table_get_float, luna_jit_table_get_int, luna_jit_table_get_int_checked,
    luna_jit_table_len, luna_jit_table_len_checked, luna_jit_table_reserve_list,
    luna_jit_table_set_checked, luna_jit_table_set_field, luna_jit_table_set_field_checked,
    luna_jit_table_set_float_float, luna_jit_table_set_int, luna_jit_table_set_int_checked,
    luna_jit_table_set_nil, luna_jit_table_set_raw, luna_jit_trace_materialize_frames,
    luna_jit_upval_get, luna_jit_upval_get_checked, luna_jit_upval_get_float,
    luna_jit_upval_of_checked,
};

#[cfg(feature = "jit-helpers")]
mod jit_helpers_pin;

/// Pull all 46 `luna_jit_*` Cranelift
/// trace-mcode helper symbols into the deploy-side staticlib's
/// linkmap. Called by the AOT-generated C `main` stub or by the
/// integration tests to make sure the helper symbols are still
/// resolvable after `cargo build -p luna-runtime-helpers --release`.
///
/// Available only when the `jit-helpers` Cargo feature is enabled
/// (default). When disabled, the staticlib excludes both
/// `luna-jit` from its dep graph and this function from its API
/// surface — interp-only AOT binaries pay zero cranelift cost.
///
/// Returns the number of helper symbols pinned (always 46 with the
/// current `luna-jit` shape; will need to be bumped in lock-step
/// any time `crates/luna-jit/src/jit_backend/mod.rs` adds a 43rd
/// `pub unsafe extern "C" fn luna_jit_*`).
///
/// # Implementation note
///
/// Re-exports alone (`pub use luna_jit::jit_backend::*`) are not
/// enough: rustc's staticlib pipeline drops `#[no_mangle]` symbols
/// from upstream rlibs unless they're reached via a kept root. The
/// `LUNA_AOT_HELPER_PIN` static + this fn together form that kept
/// root.
#[cfg(feature = "jit-helpers")]
pub fn force_link_jit_helpers() -> usize {
    jit_helpers_pin::force_link_jit_helpers()
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

#[cfg(feature = "jit-helpers")]
pub mod aot_strkey_resolver;

#[cfg(feature = "jit-helpers")]
pub mod aot_inline_chain_resolver;

#[cfg(feature = "jit-helpers")]
pub mod aot_proto_resolver;

#[cfg(feature = "jit-helpers")]
pub mod aot_trace_registry;
