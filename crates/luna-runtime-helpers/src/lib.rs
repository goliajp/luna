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

    // v1.3 Stage 7 follow-on — pin the `luna_jit_*` helper symbols
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

    // v1.3 Phase AOT Stage 7 sub-piece 3 (PENDING) — interned-string
    // slot resolver.
    //
    // Sub-piece 2 (commits adding `CompileOptions { aot: true }`)
    // changed the trace lowerer to emit data symbols of the form
    // `__luna_aot_strkey_slot_<hex>` (writable, 8-byte) and
    // `__luna_aot_strkey_bytes_<hex>` (read-only, `[u64 len ||
    // utf8...]`). The IR loads through the slot to get a
    // `Gc<LuaStr>::as_ptr()`. Slots are zero-initialised at link
    // time; reading through one without a resolver write would
    // dereference NULL on the first trace dispatch.
    //
    // Sub-piece 3 must, BEFORE `run_inner` reaches any AOT trace
    // dispatch:
    //
    // 1. Walk every `__luna_aot_strkey_bytes_*` symbol present in
    //    the link image. Two options for enumeration:
    //    a) Bracket the bytes section with linker-provided start/end
    //       symbols (`__start___luna_aot_strkey_bytes` /
    //       `__stop___luna_aot_strkey_bytes`, available on
    //       gnu-ld / lld / Mach-O via `__section$start$...`).
    //    b) Use cranelift's `Module::declare_data` to ALSO emit a
    //       small registry section listing `(slot_id, bytes_id)`
    //       pairs and walk that — strip-friendly across all targets.
    // 2. For each entry: read len from `[0..8]`, bytes from `[8..]`,
    //    call `vm.heap.intern(bytes)`, write the resulting
    //    `Gc<LuaStr>::as_ptr()` (as `i64`) into the matching slot.
    // 3. The resolver runs once, idempotent. Slots staying NULL
    //    after resolve = bug (most likely missing `_bytes_*` for a
    //    given `_slot_*`).
    //
    // Effort: 1-2 dev-days. Blocker: sub-piece 4 (trace registry +
    // dispatch install).

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

// v1.3 Stage 7 follow-on — re-export of the 27 `luna_jit_*` Cranelift
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
//   reports 27 (one per helper).
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
    luna_jit_materialize_sunk_table, luna_jit_new_table, luna_jit_new_table_sized,
    luna_jit_op_close, luna_jit_op_closure, luna_jit_op_concat, luna_jit_op_get_tab_up,
    luna_jit_op_tforcall, luna_jit_spill_to_stack, luna_jit_stack_load, luna_jit_stack_tag,
    luna_jit_stack_update_raw, luna_jit_str_buf_acquire, luna_jit_str_buf_extend,
    luna_jit_str_buf_intern, luna_jit_str_buf_release, luna_jit_table_get_field,
    luna_jit_table_get_float, luna_jit_table_get_int, luna_jit_table_len, luna_jit_table_set_field,
    luna_jit_table_set_float_float, luna_jit_table_set_int, luna_jit_table_set_nil,
    luna_jit_table_set_raw, luna_jit_trace_materialize_frames, luna_jit_upval_get,
};

#[cfg(feature = "jit-helpers")]
mod jit_helpers_pin {
    use luna_jit::jit_backend as jb;

    /// Type-erased fn-pointer slot. Cast site is link-time only —
    /// nothing in this crate actually invokes the pointers.
    type AnyFn = *const u8;

    /// SAFETY: a `*const u8` of a `fn` symbol is `Send + Sync` (the
    /// address is a process-global text-section constant). The `Sync`
    /// impl is needed so the `static` below typechecks.
    #[repr(transparent)]
    struct PinnedFn(AnyFn);
    // SAFETY: fn pointer addresses are immutable globals, safe to share
    // across threads — they're only ever read, never dereferenced.
    unsafe impl Sync for PinnedFn {}

    /// The link-anchor array. `#[used]` (and `#[unsafe(no_mangle)]` so
    /// nothing in the rustc dead-code pass can elide it across the rlib
    /// → staticlib step) tells rustc + the system linker to keep this
    /// static alive in the final object — which transitively pins each
    /// `luna_jit_*` symbol the static references.
    ///
    /// The number of entries (27) must match the number of
    /// `pub unsafe extern "C" fn luna_jit_*` in
    /// `crates/luna-jit/src/jit_backend/mod.rs`. If a future luna-jit
    /// commit adds a 28th helper, this array must grow in lock-step
    /// or AOT trace `.o`s referencing the new symbol will fail to
    /// link with `undefined reference to luna_jit_<new>`.
    #[used]
    #[unsafe(no_mangle)]
    static LUNA_AOT_HELPER_PIN: [PinnedFn; 27] = [
        PinnedFn(jb::luna_jit_new_table as AnyFn),
        PinnedFn(jb::luna_jit_new_table_sized as AnyFn),
        PinnedFn(jb::luna_jit_materialize_sunk_table as AnyFn),
        PinnedFn(jb::luna_jit_table_set_int as AnyFn),
        PinnedFn(jb::luna_jit_table_set_raw as AnyFn),
        PinnedFn(jb::luna_jit_table_set_field as AnyFn),
        PinnedFn(jb::luna_jit_table_get_field as AnyFn),
        PinnedFn(jb::luna_jit_op_get_tab_up as AnyFn),
        PinnedFn(jb::luna_jit_table_set_nil as AnyFn),
        PinnedFn(jb::luna_jit_table_set_float_float as AnyFn),
        PinnedFn(jb::luna_jit_table_get_int as AnyFn),
        PinnedFn(jb::luna_jit_table_get_float as AnyFn),
        PinnedFn(jb::luna_jit_upval_get as AnyFn),
        PinnedFn(jb::luna_jit_op_close as AnyFn),
        PinnedFn(jb::luna_jit_stack_update_raw as AnyFn),
        PinnedFn(jb::luna_jit_op_concat as AnyFn),
        PinnedFn(jb::luna_jit_str_buf_acquire as AnyFn),
        PinnedFn(jb::luna_jit_str_buf_release as AnyFn),
        PinnedFn(jb::luna_jit_str_buf_extend as AnyFn),
        PinnedFn(jb::luna_jit_str_buf_intern as AnyFn),
        PinnedFn(jb::luna_jit_op_tforcall as AnyFn),
        PinnedFn(jb::luna_jit_stack_load as AnyFn),
        PinnedFn(jb::luna_jit_stack_tag as AnyFn),
        PinnedFn(jb::luna_jit_spill_to_stack as AnyFn),
        PinnedFn(jb::luna_jit_op_closure as AnyFn),
        PinnedFn(jb::luna_jit_trace_materialize_frames as AnyFn),
        PinnedFn(jb::luna_jit_table_len as AnyFn),
    ];

    /// Pulls the link-anchor static into the public API surface so
    /// downstream `cargo build --release -p luna-runtime-helpers`
    /// keeps it through the rlib → staticlib bundling step.
    ///
    /// Returns the count of pinned helper slots. The body calls each
    /// helper through `std::hint::black_box`'d branches that are
    /// gated on an always-false runtime flag — the calls never
    /// execute, but rustc + LTO can't prove that without inlining
    /// every helper, so the call edges remain in the call graph and
    /// the staticlib bundler pulls in the cgus that define each
    /// helper.
    ///
    /// Pure-pointer references (the `LUNA_AOT_HELPER_PIN` static)
    /// alone are not enough — Rust's staticlib bundling step only
    /// picks up cgus that are reachable through the call graph, not
    /// through "address taken" graphs (verified empirically:
    /// `nm` reports `T _luna_jit_*` count = 0 when only the static
    /// references the helpers).
    ///
    /// # Safety
    ///
    /// All `luna_jit_*` helpers are `unsafe extern "C"` and must be
    /// called under an active [`luna_jit::jit_backend::enter_jit`]
    /// guard. The branches below are gated on
    /// `black_box(false)` so the calls never execute at run time;
    /// they exist solely as link-time anchors. Calling
    /// `force_link_jit_helpers` is therefore safe despite invoking
    /// `unsafe` functions inside the (unreachable) branch body.
    /// Run-time-mutable flag that defeats LTO's branch elimination on
    /// the `if NEVER.load(...) { /* call helpers */ }` guard below.
    ///
    /// `black_box(false)` alone is not enough under `lto = true` —
    /// the cross-crate LTO inliner observes the branch as dead and
    /// strips the calls (verified empirically: with the
    /// `if black_box(false)` form the cgu containing
    /// `force_link_jit_helpers` had zero `U _luna_jit_*` refs).
    ///
    /// `AtomicBool` with default `false` + `Ordering::Relaxed` load
    /// is opaque to LTO — the optimizer cannot prove the atomic is
    /// never written by another translation unit, so the branch
    /// survives. The atomic IS never written (nobody calls a
    /// setter), so the branch is dynamically dead at run time.
    static NEVER_TRIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    #[allow(unreachable_code)]
    pub fn force_link_jit_helpers() -> usize {
        // Address-table touch keeps `LUNA_AOT_HELPER_PIN` live.
        let mut sum: usize = 0;
        for slot in LUNA_AOT_HELPER_PIN.iter() {
            sum = sum.wrapping_add(std::hint::black_box(slot.0 as usize));
        }

        // Call-graph anchor — gated by an atomic load LTO can't
        // constant-fold. Branch never executes at run time
        // (`NEVER_TRIP` is never written), but the call edges to each
        // `luna_jit_*` helper survive into the staticlib bundling.
        if NEVER_TRIP.load(std::sync::atomic::Ordering::Relaxed) {
            // SAFETY: the surrounding `if black_box(false)` is
            // never entered at run time. The calls exist solely to
            // pin the helper symbols' cgus into the staticlib
            // bundling step's reachable set.
            unsafe {
                let _ = jb::luna_jit_new_table();
                let _ = jb::luna_jit_new_table_sized(0);
                let _ = jb::luna_jit_materialize_sunk_table(
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                );
                jb::luna_jit_table_set_int(0, 0, 0);
                jb::luna_jit_table_set_raw(0, 0, 0, 0);
                jb::luna_jit_table_set_field(0, 0, 0, 0);
                let _ = jb::luna_jit_table_get_field(0, 0);
                let _ = jb::luna_jit_op_get_tab_up(0, 0);
                jb::luna_jit_table_set_nil(0, 0);
                jb::luna_jit_table_set_float_float(0, 0, 0);
                let _ = jb::luna_jit_table_get_int(0, 0);
                let _ = jb::luna_jit_table_get_float(0, 0);
                let _ = jb::luna_jit_upval_get(0);
                let _ = jb::luna_jit_op_close(0);
                jb::luna_jit_stack_update_raw(0, 0);
                let _ = jb::luna_jit_op_concat(0, 0);
                let _ = jb::luna_jit_str_buf_acquire();
                jb::luna_jit_str_buf_release(0);
                let _ = jb::luna_jit_str_buf_extend(0, 0);
                let _ = jb::luna_jit_str_buf_intern(0);
                let _ = jb::luna_jit_op_tforcall(
                    0,
                    0,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                let _ = jb::luna_jit_stack_load(0);
                let _ = jb::luna_jit_stack_tag(0);
                jb::luna_jit_spill_to_stack(0, 0, 0);
                let _ = jb::luna_jit_op_closure(0);
                let _ = jb::luna_jit_trace_materialize_frames(0, std::ptr::null());
                let _ = jb::luna_jit_table_len(0);
            }
        }

        std::hint::black_box(sum);
        LUNA_AOT_HELPER_PIN.len()
    }
}

/// v1.3 Stage 7 follow-on — pull all 27 `luna_jit_*` Cranelift
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
/// Returns the number of helper symbols pinned (always 27 with the
/// current `luna-jit` shape; will need to be bumped in lock-step
/// any time `crates/luna-jit/src/jit_backend/mod.rs` adds a 28th
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
