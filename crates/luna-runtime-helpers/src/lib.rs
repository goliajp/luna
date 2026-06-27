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

// v1.3 Phase AOT Stage 7 polish 3 — Windows PE/COFF section walker.
// Used by `aot_strkey_resolver` and `aot_trace_registry` to enumerate
// the deploy-side `lt_meta` / `lt_skix` sections on Windows, where
// the Unix-style `__start_/__stop_` bracket symbol convention isn't
// synthesized by `link.exe` / `lld-link`. Hand-rolled winapi externs
// keep the dep story unchanged (no `windows-sys` / `winapi` crate
// added). See module docs for the design rationale.
#[cfg(all(target_os = "windows", feature = "jit-helpers"))]
mod windows_section;

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

    // v1.3 Phase AOT Stage 7 trace-coverage follow-up — install the
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
        // NOTE: `trace_enabled = true` (the TA3 ship default) is
        // load-bearing for AOT dispatch too — `Vm::run`'s trace
        // lookup gate is `if self.jit.trace_enabled`, used for BOTH
        // runtime-compiled traces AND AOT-installed traces.
        // Disabling here would silently skip the AOT install's
        // dispatch. Runtime re-recording for back-edges the AOT
        // didn't cover is fine — same pattern interp + JIT uses.
    }

    // v1.3 Phase AOT Stage 7 sub-piece 3 — interned-string slot
    // resolver. Runs BEFORE `vm.load` so the resulting closure's
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
        // v1.3 Phase AOT Stage 7 polish 6 — inline chain slot
        // population. Must run BEFORE `aot_trace_registry::install_all`
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

    // v1.3 Phase AOT Stage 7 sub-piece 4 — install AOT-emitted traces
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
        // SAFETY: closure is a live Gc<LuaClosure>; reading .proto is
        // a NonNull pointer copy. The heap is single-threaded so no
        // concurrent mutation is possible during this read.
        let root_proto = unsafe { (*closure.as_ptr()).proto };
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

    // v2.0 Phase 5 Track AO sub-track AO-PF — post-run probe for the
    // Stage 7 polish 6 inline-chain reloc fire path. Counts every
    // entry to `luna_jit_trace_materialize_frames` from trace mcode
    // (JIT-baked OR AOT polish-6 slot-loaded). In an AOT-only binary
    // any non-zero value is direct evidence that the polish-6 chain
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

/// v1.3 Phase AOT Stage 7 sub-piece 3 — deploy-side interned-string
/// slot resolver.
///
/// AOT trace mcode emitted by [`luna_jit::jit_backend::trace::
/// lower_trace_into`] with `CompileOptions { aot: true }` reads
/// interned-string-key pointers indirectly through writable 8-byte
/// slots (`__luna_aot_strkey_slot_<hex>`). Each unique key
/// contributes a 16-byte `[bytes_addr, slot_addr]` entry to a
/// dedicated `luna_strkey_idx` section. The deploy binary's static
/// linker auto-brackets that section via
/// `__start_luna_strkey_idx` / `__stop_luna_strkey_idx` (ELF) or
/// `section$start$__DATA$luna_strkey_idx` /
/// `section$end$__DATA$luna_strkey_idx` (Mach-O), and this resolver
/// walks the bracketed range to: (a) intern each bytes block into
/// the deploy `Vm`'s heap, and (b) write the resulting
/// `Gc<LuaStr>::as_ptr()` into the matching slot.
///
/// # Safety contract (called from `run_inner` only)
///
/// - Must run **once**, BEFORE the deploy `Vm` dispatches into any
///   AOT mcode. `run_inner` calls it after `Vm::new` /
///   `set_bytecode_loading` and before `vm.load`.
/// - Idempotent under second-call: the slots already hold valid
///   `Gc<LuaStr>` pointers, the section walk re-interns the bytes
///   (cheap — string-table dedup), and re-writes the slot with the
///   same pointer.
/// - Empty-section tolerant: a deploy binary that linked zero AOT
///   trace `.o`s has both bracket symbols collapsing to the same
///   address; the walk loop terminates with zero entries.
///
/// # Why feature-gated on `jit-helpers`
///
/// The whole AOT-trace path is jit-helper-gated. With
/// `default-features = false` the staticlib excludes
/// `luna-jit` and the resolver becomes a no-op — interp-only AOT
/// binaries pay zero scan cost and the bracket-symbol references are
/// elided.
#[cfg(feature = "jit-helpers")]
pub mod aot_strkey_resolver {
    use luna_core::vm::Vm;

    /// Index entry layout — must match the cranelift-emit shape in
    /// `crates/luna-jit/src/jit_backend/trace.rs::emit_str_key_arg`:
    /// two pointer-sized fields, `bytes_ptr` and `slot_ptr`, both
    /// resolved by the static linker before process load completes.
    #[repr(C)]
    struct IndexEntry {
        /// Address of the `__luna_aot_strkey_bytes_<hex>` symbol:
        /// `[u64 len | utf8...]` payload, read-only.
        bytes_ptr: *const u8,
        /// Address of the `__luna_aot_strkey_slot_<hex>` symbol:
        /// writable 8-byte slot, zero-initialised at link time, this
        /// resolver writes the interned `Gc<LuaStr>` pointer in.
        slot_ptr: *mut *const u8,
    }

    // ELF / lld auto-creates `__start_<name>` / `__stop_<name>` for
    // sections whose name is a valid C identifier. Our section is
    // `luna_strkey_idx` (set via cranelift's `set_segment_section`).
    //
    // Mach-O uses a different convention: `section$start$<seg>$<sect>`
    // / `section$end$<seg>$<sect>`, synthesized by Apple `ld`. We
    // declare per-platform externs and the dead-strip pass discards
    // whichever doesn't match.
    //
    // Windows / COFF has no bracket-symbol convention (Stage 7 polish
    // 3): `link.exe` / `lld-link` don't synthesize `__start_` / `__stop_`
    // externs. Instead the deploy walker calls into the parent crate's
    // [`crate::windows_section::find_section`] which does a runtime
    // PE-header parse via `GetModuleHandleW(NULL)`. The Windows path
    // uses the short section name `.lt_skix` (8 bytes, COFF
    // section-name max) — see `crates/luna-aot/src/embed.rs` for the
    // emit-side choice. Empty-section (no AOT traces linked in) is
    // handled by `find_section` returning `None` and `resolve_all`
    // short-circuiting to 0.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    unsafe extern "C" {
        #[link_name = "__start_luna_strkey_idx"]
        static mut LUNA_STRKEY_IDX_START: u8;
        #[link_name = "__stop_luna_strkey_idx"]
        static mut LUNA_STRKEY_IDX_END: u8;
    }

    #[cfg(target_vendor = "apple")]
    unsafe extern "C" {
        #[link_name = "\u{1}section$start$__DATA$luna_strkey_idx"]
        static mut LUNA_STRKEY_IDX_START: u8;
        #[link_name = "\u{1}section$end$__DATA$luna_strkey_idx"]
        static mut LUNA_STRKEY_IDX_END: u8;
    }

    /// Walk the bracketed `luna_strkey_idx` section (Unix / Mach-O) or
    /// the PE-header-located `.lt_skix` section (Windows), intern each
    /// bytes block into `vm.heap`, write the resulting pointer into
    /// the matching slot. Returns the number of slots populated
    /// (zero on a binary that linked zero AOT trace `.o`s).
    pub fn resolve_all(vm: &mut Vm) -> usize {
        // Locate the strkey-idx section + length, dispatching on
        // target platform. Windows: runtime PE header walk via
        // [`crate::windows_section::find_section`] for the short name
        // `.lt_skix` (mirrors the emit-side choice in
        // `crates/luna-aot/src/embed.rs::write_aot_cmain_object_for`
        // Windows arm + the harvester's `set_segment_section`).
        // Unix/Mach-O: bracket symbols supplied by the linker. Either
        // dispatch path can produce a zero-length section (binary
        // linked no AOT traces) — `walk_index_bytes` short-circuits.
        let (base, len_bytes): (*const u8, usize) = {
            #[cfg(target_os = "windows")]
            {
                match crate::windows_section::find_section(b".lt_skix") {
                    Some((b, l)) => (b, l),
                    None => return 0,
                }
            }
            #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
            {
                let start = &raw mut LUNA_STRKEY_IDX_START as *mut IndexEntry;
                let end = &raw mut LUNA_STRKEY_IDX_END as *mut IndexEntry;
                // start == end on a binary with zero AOT traces. Section
                // length = end - start in bytes; divide by entry size
                // gives the count.
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
            {
                let start = &raw mut LUNA_STRKEY_IDX_START as *mut IndexEntry;
                let end = &raw mut LUNA_STRKEY_IDX_END as *mut IndexEntry;
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            // Platforms without a section enumeration path (e.g.
            // wasm32) — no AOT install possible, return 0.
            #[cfg(not(any(
                target_os = "windows",
                all(unix, not(target_vendor = "apple")),
                target_vendor = "apple"
            )))]
            {
                let _ = vm;
                return 0;
            }
        };
        walk_index_bytes(vm, base, len_bytes)
    }

    /// Common per-entry walk shared by the Unix/Mach-O bracket-symbol
    /// path and the Windows PE-header-located section path. Takes the
    /// section base + length in bytes (as the two enumeration paths
    /// produce different types — a pair of bracket symbol addresses on
    /// Unix, a `(*const u8, usize)` tuple from [`crate::windows_section
    /// ::find_section`] on Windows) and walks `len / sizeof(IndexEntry)`
    /// entries.
    ///
    /// Tolerant of trailing-zero placeholder entries (the cmain shim
    /// emits one zero-filled IndexEntry to guarantee the section exists
    /// even when no real traces are linked in) via the
    /// `entry.bytes_ptr.is_null() || entry.slot_ptr.is_null()` skip.
    fn walk_index_bytes(vm: &mut Vm, base: *const u8, len_bytes: usize) -> usize {
        if base.is_null() || len_bytes == 0 {
            return 0;
        }
        let n_entries = len_bytes / core::mem::size_of::<IndexEntry>();
        let start = base as *const IndexEntry;
        let mut populated = 0usize;
        // SAFETY: caller guarantees `[base, base + len_bytes)` is
        // mapped readable memory owned by a linker-defined section.
        // Each IndexEntry read is bounded by n_entries. Slot writes
        // target the `slot_ptr` field which the lowerer guarantees
        // points at a writable 8-byte slot in the same image.
        unsafe {
            for i in 0..n_entries {
                let entry = &*start.add(i);
                if entry.bytes_ptr.is_null() || entry.slot_ptr.is_null() {
                    continue;
                }
                let len = core::ptr::read_unaligned(entry.bytes_ptr as *const u64) as usize;
                let payload = entry.bytes_ptr.add(8);
                let bytes = core::slice::from_raw_parts(payload, len);
                let interned = vm.heap.intern(bytes);
                core::ptr::write(entry.slot_ptr, interned.as_ptr() as *const u8);
                populated += 1;
            }
        }
        populated
    }
}

// v1.3 Phase AOT Stage 7 polish 6 — deploy-side inline-chain resolver.
//
// Mirrors `aot_strkey_resolver`'s shape. The trace lowerer's
// `emit_chain_ptr_arg` (`crates/luna-jit/src/jit_backend/trace.rs`)
// emits one `(slot, bytes, idx)` triple per unique
// `FrameMaterializeInfo` chain when `opts.aot == true`; this resolver
// walks the bracketed `luna_inline_chnx` section (Unix / Mach-O) or the
// PE-header-located `.lt_chai` section (Windows), parses each bytes
// payload into a `Vec<FrameMaterializeInfo>`, leaks it as a
// process-lifetime `Rc<[...]>` (so the IR's load yields a valid pointer
// for the binary's lifetime — there is no per-trace tear-down on the
// AOT path), and writes the chain's first-element pointer into the
// matching slot.
//
// Why a separate Rc instead of pointing the slot at the bytes section
// directly:
//   - The IR loads the slot, then passes the value as `*const
//     FrameMaterializeInfo` to `luna_jit_trace_materialize_frames`,
//     which interprets the bytes as a fully-aligned array of `repr(C)`
//     structs. The bytes section is already 8-byte aligned with the
//     same packing, so a `bytes_ptr + 8` (skip the count prefix) would
//     work, but a future change to `FrameMaterializeInfo` layout would
//     silently misinterpret stale bytes. Going through an explicit
//     `Vec → Rc<[...]>` conversion lets us validate `chain_bytes.len()
//     % 12 == 0` (via the same `PerExitInlineEntry::FRAME_MATERIALIZE
//     _INFO_SIZE` constant the v3 wire format uses) and surface
//     corruption with a probe message rather than dispatching into
//     garbage.
//   - Keeping the chain's ownership on the Rust side mirrors the JIT
//     path's `per_exit_inline_vec.push((..., chain_rc, ...))` — the
//     dispatcher's `CompiledTrace::per_exit_inline[i].chain` field can
//     hold its own Rc rebuilt from the same bytes (decoded by the
//     trace install path); the IR pointer and the dispatcher field
//     point at independently-allocated copies of the same data, but
//     neither side compares pointers, only reads through them.
#[cfg(feature = "jit-helpers")]
pub mod aot_inline_chain_resolver {
    //! v1.3 Phase AOT Stage 7 polish 6 — `FrameMaterializeInfo` chain
    //! pointer reloc resolver. See parent-module preamble for the full
    //! design rationale; this module owns the deploy-side walk +
    //! per-chain Rc materialization + slot write.
    use luna_core::jit::trace_types::FrameMaterializeInfo;

    /// Wire-size of one `FrameMaterializeInfo` record on disk and in
    /// the bytes section payload. Asserted at compile time in
    /// `luna_core::jit::aot_meta::FRAME_MATERIALIZE_INFO_WIRE_SIZE_CHECK`.
    const FRAME_MATERIALIZE_INFO_SIZE: usize = 12;

    /// Index entry layout — must match the cranelift-emit shape in
    /// `crates/luna-jit/src/jit_backend/trace.rs::emit_chain_ptr_arg`:
    /// two pointer-sized fields, `bytes_ptr` and `slot_ptr`, both
    /// resolved by the static linker before process load completes.
    #[repr(C)]
    struct IndexEntry {
        /// Address of the `__luna_aot_inline_chain_bytes_<hex>` symbol:
        /// `[u64 count | packed_records...]` payload, read-only. The
        /// records are tightly packed 12-byte
        /// `(base_offset, pc, nresults)` triples.
        bytes_ptr: *const u8,
        /// Address of the `__luna_aot_inline_chain_slot_<hex>` symbol:
        /// writable 8-byte slot, zero-initialised at link time. This
        /// resolver writes the leaked chain's first-element pointer
        /// here.
        slot_ptr: *mut *const FrameMaterializeInfo,
    }

    // ELF / lld auto-creates `__start_<name>` / `__stop_<name>` for
    // sections whose name is a valid C identifier (`luna_inline_chnx`).
    // Mach-O uses `section$start$<seg>$<sect>` /
    // `section$end$<seg>$<sect>`, synthesised by Apple `ld`. Windows
    // COFF has neither — see the runtime PE-header walker in the
    // `resolve_all` arm.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    unsafe extern "C" {
        #[link_name = "__start_luna_inline_chnx"]
        static mut LUNA_INLINE_CHNX_START: u8;
        #[link_name = "__stop_luna_inline_chnx"]
        static mut LUNA_INLINE_CHNX_END: u8;
    }

    #[cfg(target_vendor = "apple")]
    unsafe extern "C" {
        #[link_name = "\u{1}section$start$__DATA$luna_inline_chnx"]
        static mut LUNA_INLINE_CHNX_START: u8;
        #[link_name = "\u{1}section$end$__DATA$luna_inline_chnx"]
        static mut LUNA_INLINE_CHNX_END: u8;
    }

    /// Walk the bracketed `luna_inline_chnx` section (Unix / Mach-O) or
    /// the PE-header-located `.lt_chai` section (Windows). For each
    /// entry: rebuild a `Vec<FrameMaterializeInfo>` from the bytes
    /// payload, materialise as a `Rc<[...]>`, leak ownership (process-
    /// lifetime — AOT traces never tear down), write the chain's
    /// first-element pointer into the matching slot.
    ///
    /// Returns the number of slots populated (zero on a binary that
    /// linked zero AOT traces with inline cmp@d>0 side-exits).
    ///
    /// Tolerant of trailing-zero placeholder entries (the cmain shim
    /// emits one zero-filled IndexEntry to guarantee the section exists
    /// even when no real chain symbols are linked) via the null-pointer
    /// guard in `walk_index_bytes`.
    pub fn resolve_all() -> usize {
        let (base, len_bytes): (*const u8, usize) = {
            #[cfg(target_os = "windows")]
            {
                match crate::windows_section::find_section(b".lt_chai") {
                    Some((b, l)) => (b, l),
                    None => return 0,
                }
            }
            #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
            {
                let start = &raw mut LUNA_INLINE_CHNX_START as *mut IndexEntry;
                let end = &raw mut LUNA_INLINE_CHNX_END as *mut IndexEntry;
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
            {
                let start = &raw mut LUNA_INLINE_CHNX_START as *mut IndexEntry;
                let end = &raw mut LUNA_INLINE_CHNX_END as *mut IndexEntry;
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            #[cfg(not(any(
                target_os = "windows",
                all(unix, not(target_vendor = "apple")),
                target_vendor = "apple"
            )))]
            {
                return 0;
            }
        };
        walk_index_bytes(base, len_bytes)
    }

    /// Common per-entry walk shared by the Unix/Mach-O bracket-symbol
    /// path and the Windows PE-header-located section path.
    ///
    /// Each entry's `bytes_ptr` points at `[u64 count, records...]`;
    /// we decode the count, validate `count * 12` doesn't overflow,
    /// parse `count` `FrameMaterializeInfo` triples, materialise them
    /// as an `Rc<[FrameMaterializeInfo]>`, leak ownership via
    /// `core::mem::forget(rc.clone())` (the inner buffer stays alive
    /// for process lifetime), and write the first-element pointer into
    /// the slot. Subsequent AOT mcode dispatches read the slot and pass
    /// the pointer to `luna_jit_trace_materialize_frames(n, ptr)`.
    ///
    /// Corrupt entries (null pointers, unaligned count, count overflow)
    /// are skipped silently with an `LUNA_AOT_PROBE` line on stderr —
    /// the trace's first inline side-exit dispatch will then deopt via
    /// the helper's `pending_err` path because the slot stays NULL.
    fn walk_index_bytes(base: *const u8, len_bytes: usize) -> usize {
        if base.is_null() || len_bytes == 0 {
            return 0;
        }
        let probe_on = std::env::var_os("LUNA_AOT_PROBE").is_some();
        let n_entries = len_bytes / core::mem::size_of::<IndexEntry>();
        let start = base as *const IndexEntry;
        let mut populated = 0usize;
        // SAFETY: caller guarantees `[base, base + len_bytes)` is
        // mapped readable memory owned by a linker-defined section.
        // Each IndexEntry read is bounded by n_entries. Bytes-payload
        // reads are bounded by the per-entry count (validated for
        // overflow before the slice constructor). Slot writes target
        // the `slot_ptr` field which the lowerer guarantees points at
        // a writable 8-byte slot in the same image.
        unsafe {
            for i in 0..n_entries {
                let entry = &*start.add(i);
                if entry.bytes_ptr.is_null() || entry.slot_ptr.is_null() {
                    continue;
                }
                // Bytes layout: little-endian u64 record count, then
                // `count * FRAME_MATERIALIZE_INFO_SIZE` packed bytes.
                let count = core::ptr::read_unaligned(entry.bytes_ptr as *const u64) as usize;
                let Some(bytes_len) = count.checked_mul(FRAME_MATERIALIZE_INFO_SIZE) else {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_inline_chain skip entry {i} reason=count_overflow count={count}"
                        );
                    }
                    continue;
                };
                let payload = entry.bytes_ptr.add(8);
                let raw = core::slice::from_raw_parts(payload, bytes_len);
                let mut vec: Vec<FrameMaterializeInfo> = Vec::with_capacity(count);
                for j in 0..count {
                    let off = j * FRAME_MATERIALIZE_INFO_SIZE;
                    let base_offset = u32::from_le_bytes(raw[off..off + 4].try_into().unwrap());
                    let pc = u32::from_le_bytes(raw[off + 4..off + 8].try_into().unwrap());
                    let nresults = i32::from_le_bytes(raw[off + 8..off + 12].try_into().unwrap());
                    vec.push(FrameMaterializeInfo {
                        base_offset,
                        pc,
                        nresults,
                    });
                }
                let rc: luna_core::jit::send_compat::TArc<[FrameMaterializeInfo]> = vec.into();
                // `Rc<[T]>::as_ptr` returns a fat `*const [T]`; the
                // first-element address is what the IR's
                // `luna_jit_trace_materialize_frames` consumes. For a
                // non-empty chain, `rc[0]` is the data pointer; for an
                // empty chain (count == 0) the IR never reaches the
                // helper (the side-exit's `if !call_chain.is_empty()`
                // gate at compile time would have routed through the
                // d=0 arm), so the slot stays at a dangling-but-unused
                // value. Guard anyway for paranoia.
                let chain_ptr: *const FrameMaterializeInfo = if count == 0 {
                    core::ptr::null()
                } else {
                    &rc[0] as *const FrameMaterializeInfo
                };
                // Leak ownership so the chain bytes stay alive for the
                // process. AOT-installed traces never tear down (no
                // `proto.traces.borrow_mut().remove(...)` path on
                // deploy), so a single leak per unique chain matches
                // the lifetime requirement exactly.
                core::mem::forget(rc);
                core::ptr::write(entry.slot_ptr, chain_ptr);
                populated += 1;
            }
        }
        populated
    }
}

// v1.3 Phase AOT Stage 7 sub-piece 4 — trace dispatch registry.
// See module-level docs inside the block for the deploy-side walker
// shape; the AOT-compile-side recorder + emitter lives in
// `crates/luna-aot/src/embed.rs::harvest_and_emit_aot_traces`.
#[cfg(feature = "jit-helpers")]
pub mod aot_trace_registry {
    //! v1.3 Phase AOT Stage 7 sub-piece 4 — deploy-side trace-meta
    //! walker.
    //!
    //! `luna-aot::embed::harvest_and_emit_aot_traces` emits a 48-byte
    //! [`luna_core::jit::aot_meta::AotTraceIndexEntry`] per AOT-installable
    //! trace into the `luna_trace_meta` bracketed section, plus a
    //! combined meta-blob payload in `luna_trace_blob`. This walker
    //! runs once at startup (between `vm.set_bytecode_loading(true)`
    //! and `vm.load`), iterates the bracket-bounded section, matches
    //! each entry's `proto_hash` against the loaded chunk's proto
    //! tree via [`Vm::collect_proto_hashes`], and calls
    //! [`Vm::install_aot_trace`] with a freshly constructed
    //! [`CompiledTrace`] whose `entry` points at the linker-resolved
    //! AOT mcode.
    //!
    //! Empty-section tolerant: a binary with zero linked trace `.o`s
    //! has both bracket symbols collapse to the same address; the walk
    //! short-circuits with `Ok(0)`.

    use luna_core::jit::aot_meta::{
        AotTraceIndexEntry, decode_meta_blob, unpack_exit_tag, unpack_tag_res_kind,
    };
    use luna_core::jit::trace_types::{CompiledTrace, ExitTag, TraceFn};
    use luna_core::vm::Vm;

    // Bracket symbols — same pattern as sub-piece 3's strkey_idx
    // walker. ELF / lld auto-create `__start_<name>` / `__stop_<name>`
    // for sections whose name is a valid C identifier; Mach-O uses
    // `section$start$<seg>$<sect>` / `section$end$<seg>$<sect>`.
    //
    // Windows COFF has no bracket-symbol convention (Stage 7 polish 3):
    // the Windows path uses a runtime PE-header walk via
    // [`crate::windows_section::find_section`] for the short-name
    // section `.lt_meta` instead. See `windows_section` module docs.
    #[cfg(all(unix, not(target_vendor = "apple")))]
    unsafe extern "C" {
        #[link_name = "__start_luna_trace_meta"]
        static mut LUNA_TRACE_META_START: u8;
        #[link_name = "__stop_luna_trace_meta"]
        static mut LUNA_TRACE_META_END: u8;
    }

    #[cfg(target_vendor = "apple")]
    unsafe extern "C" {
        #[link_name = "\u{1}section$start$__DATA$luna_trace_meta"]
        static mut LUNA_TRACE_META_START: u8;
        #[link_name = "\u{1}section$end$__DATA$luna_trace_meta"]
        static mut LUNA_TRACE_META_END: u8;
    }

    /// Walk the `luna_trace_meta` section, install one `CompiledTrace`
    /// per entry whose `proto_hash` matches a Proto reachable from
    /// `root`. Returns the count installed.
    ///
    /// Entries whose meta blob fails to decode (magic / version
    /// mismatch, truncation) are skipped silently — the trace falls
    /// back to JIT at runtime. `LUNA_AOT_PROBE=1` surfaces the count
    /// + per-entry skip reasons on stderr for diagnosis.
    ///
    /// The deploy `Vm` never side-traces an AOT-installed parent
    /// (recorder is invoked from the dispatch path; AOT install
    /// happens BEFORE the first dispatch), so the bare
    /// [`CompiledTrace::from_aot_meta`] constructor with empty
    /// `per_exit_inline` / `per_exit_tags` is sufficient.
    pub fn install_all(
        vm: &mut Vm,
        root: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
    ) -> usize {
        // Locate the trace-meta section + length, dispatching on
        // target platform. Unix/Mach-O: linker-synthesised bracket
        // symbols. Windows: runtime PE-header walk via
        // [`crate::windows_section::find_section`] (Stage 7 polish 3)
        // for the short name `.lt_meta`. Either path can produce a
        // zero-length section (binary linked no AOT traces) —
        // `walk_meta_section` short-circuits.
        let (base, len_bytes): (*const u8, usize) = {
            #[cfg(target_os = "windows")]
            {
                match crate::windows_section::find_section(b".lt_meta") {
                    Some((b, l)) => (b, l),
                    None => return 0,
                }
            }
            #[cfg(all(not(target_os = "windows"), unix, not(target_vendor = "apple")))]
            {
                let start = &raw mut LUNA_TRACE_META_START as *mut AotTraceIndexEntry;
                let end = &raw mut LUNA_TRACE_META_END as *mut AotTraceIndexEntry;
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            #[cfg(all(not(target_os = "windows"), target_vendor = "apple"))]
            {
                let start = &raw mut LUNA_TRACE_META_START as *mut AotTraceIndexEntry;
                let end = &raw mut LUNA_TRACE_META_END as *mut AotTraceIndexEntry;
                let len = (end as isize) - (start as isize);
                if len <= 0 {
                    return 0;
                }
                (start as *const u8, len as usize)
            }
            #[cfg(not(any(
                target_os = "windows",
                all(unix, not(target_vendor = "apple")),
                target_vendor = "apple"
            )))]
            {
                let _ = (vm, root);
                return 0;
            }
        };
        // SAFETY: `(base, len_bytes)` was produced either by a linker-
        // synthesised bracket symbol pair bounding a contiguous run of
        // `AotTraceIndexEntry`, or by [`windows_section::find_section`]
        // which returns the section's run-time base + virtual_size for
        // a PE section we ourselves emit via cranelift_object. Both
        // shapes satisfy `walk_meta_section`'s unsafe contract.
        unsafe { walk_meta_section(vm, root, base as *const AotTraceIndexEntry, len_bytes) }
    }

    /// Common walk shared by the Unix/Mach-O bracket-symbol path and
    /// the Windows PE-header-located section path. Iterates one
    /// [`AotTraceIndexEntry`] at a time, decoding the meta blob and
    /// installing on the matched proto.
    ///
    /// # Safety
    ///
    /// `start` must point at the first byte of a `len_bytes`-long
    /// run of `AotTraceIndexEntry` instances, all in readable memory
    /// (linker-defined section or PE-mapped section data). Per-entry
    /// `meta_ptr` / `fn_ptr` are validated by the per-entry null
    /// checks; meta blob bytes are bounded by the entry's `meta_len`.
    unsafe fn walk_meta_section(
        vm: &mut Vm,
        root: luna_core::runtime::Gc<luna_core::runtime::function::Proto>,
        start: *const AotTraceIndexEntry,
        len_bytes: usize,
    ) -> usize {
        if start.is_null() || len_bytes < core::mem::size_of::<AotTraceIndexEntry>() {
            return 0;
        }
        let n_entries = len_bytes / core::mem::size_of::<AotTraceIndexEntry>();
        let proto_hashes = vm.collect_proto_hashes(root);
        let probe_on = std::env::var_os("LUNA_AOT_PROBE").is_some();
        let mut installed = 0usize;
        // SAFETY: caller invariant — [start, start+n_entries) is
        // mapped readable memory containing valid AotTraceIndexEntry
        // instances or zero-fill placeholder bytes (skipped via the
        // fn_ptr == 0 guard).
        unsafe {
            for i in 0..n_entries {
                let entry = &*start.add(i);
                // The placeholder entry in `luna_trace_meta` is a single
                // zero byte from the cmain shim — the section walk steps
                // past it via the size-rounding above. An entry whose
                // `fn_ptr` is null came from that placeholder and must
                // be skipped (not from a real AOT-emitted trace).
                if entry.fn_ptr == 0 || entry.meta_ptr == 0 {
                    continue;
                }
                let meta_bytes = core::slice::from_raw_parts(
                    entry.meta_ptr as *const u8,
                    entry.meta_len as usize,
                );
                let decoded = match decode_meta_blob(meta_bytes) {
                    Ok(d) => d,
                    Err(reason) => {
                        if probe_on {
                            eprintln!(
                                "luna-runtime-helpers: aot_trace skip head_pc={} reason={reason}",
                                entry.head_pc
                            );
                        }
                        continue;
                    }
                };
                // Find the matching Proto by hash.
                let matched = proto_hashes
                    .iter()
                    .find(|(_p, h)| *h == entry.proto_hash)
                    .map(|(p, _h)| *p);
                let Some(proto) = matched else {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_trace skip head_pc={} reason=proto_hash_unmatched",
                            entry.head_pc
                        );
                    }
                    continue;
                };
                // Reconstruct exit_tags + entry_tags + global_tag_res_kind.
                let mut exit_tags_vec: Vec<ExitTag> = Vec::with_capacity(decoded.exit_tags.len());
                let mut tag_decode_ok = true;
                for raw in decoded.exit_tags.iter().copied() {
                    if let Some(t) = unpack_exit_tag(raw) {
                        exit_tags_vec.push(t);
                    } else {
                        tag_decode_ok = false;
                        break;
                    }
                }
                let Some(tag_res_kind) = unpack_tag_res_kind(decoded.header.tag_res_kind) else {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_trace skip head_pc={} reason=tag_res_kind_invalid",
                            entry.head_pc
                        );
                    }
                    continue;
                };
                if !tag_decode_ok {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_trace skip head_pc={} reason=exit_tag_invalid",
                            entry.head_pc
                        );
                    }
                    continue;
                }
                let entry_tags_rc: luna_core::jit::send_compat::TArc<[u8]> = decoded.entry_tags.into();
                let exit_tags_rc: luna_core::jit::send_compat::TArc<[ExitTag]> = exit_tags_vec.into();
                // v2 per_exit_tags decode: reconstruct
                // `Vec<(cont_pc, Rc<[ExitTag]>)>` matching the
                // dispatcher's `decode_exit_shape` shape lookup. Each
                // entry's packed-byte `ExitTag` array unpacks via
                // [`unpack_exit_tag`]; an invalid byte = skip the
                // whole trace (matches the existing exit-tag handling).
                let mut per_exit_tags_decoded: Vec<(u32, luna_core::jit::send_compat::TArc<[ExitTag]>)> =
                    Vec::with_capacity(decoded.per_exit_tags.len());
                let mut per_exit_tags_ok = true;
                for ent in &decoded.per_exit_tags {
                    let mut tags: Vec<ExitTag> = Vec::with_capacity(ent.tags_packed.len());
                    for raw in ent.tags_packed.iter().copied() {
                        if let Some(t) = unpack_exit_tag(raw) {
                            tags.push(t);
                        } else {
                            per_exit_tags_ok = false;
                            break;
                        }
                    }
                    if !per_exit_tags_ok {
                        break;
                    }
                    per_exit_tags_decoded.push((ent.cont_pc, tags.into()));
                }
                if !per_exit_tags_ok {
                    if probe_on {
                        eprintln!(
                            "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_tag_invalid",
                            entry.head_pc
                        );
                    }
                    continue;
                }
                // v1.3 Phase AOT Stage 7 polish 6 — v3 per_exit_inline
                // decode is NOW load-bearing. Each wire entry's
                // `chain_bytes` rebuilds into a fresh
                // `Vec<FrameMaterializeInfo>` → `Rc<[...]>` for the
                // dispatcher's `per_exit_inline[i].chain` field; the
                // `tags_packed` array unpacks through `unpack_exit_tag`
                // into `Rc<[ExitTag]>` (same shape as the v2
                // per_exit_tags pattern above). A failing chain rebuild
                // (length not a multiple of 12 — corruption) or an
                // invalid packed tag (out-of-range byte) means the
                // trace skips install; the trace then falls back to
                // JIT at runtime via the recorder.
                //
                // The IR-baked chain pointer lives in a separate slot
                // populated by `aot_inline_chain_resolver::resolve_all`
                // (called from `run_inner` BEFORE this install path).
                // The two chain owners (this Rc and the leaked Rc
                // behind the slot) are independent allocations of the
                // same byte content — neither side compares pointers.
                let mut per_exit_inline_decoded: Vec<luna_core::jit::trace_types::InlineSideExit> =
                    Vec::with_capacity(decoded.per_exit_inline.len());
                let mut inline_ok = true;
                for ent in &decoded.per_exit_inline {
                    let Some(chain_vec) = ent.rebuild_chain() else {
                        inline_ok = false;
                        if probe_on {
                            eprintln!(
                                "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_chain_invalid (cont_pc={})",
                                entry.head_pc, ent.cont_pc
                            );
                        }
                        break;
                    };
                    let mut tags: Vec<ExitTag> = Vec::with_capacity(ent.tags_packed.len());
                    for raw in ent.tags_packed.iter().copied() {
                        if let Some(t) = unpack_exit_tag(raw) {
                            tags.push(t);
                        } else {
                            inline_ok = false;
                            break;
                        }
                    }
                    if !inline_ok {
                        if probe_on {
                            eprintln!(
                                "luna-runtime-helpers: aot_trace skip head_pc={} reason=per_exit_inline_tag_invalid (cont_pc={})",
                                entry.head_pc, ent.cont_pc
                            );
                        }
                        break;
                    }
                    per_exit_inline_decoded.push(luna_core::jit::trace_types::InlineSideExit {
                        cont_pc: ent.cont_pc,
                        head_resume_pc: ent.head_resume_pc,
                        exit_tags: tags.into(),
                        chain: chain_vec.into(),
                        side_trace_ptr: Box::new(luna_core::jit::send_compat::TCellPtr::null()),
                    });
                }
                if !inline_ok {
                    continue;
                }
                // Transmute the C-ABI fn ptr from u64 (wire-width-
                // stable) to `TraceFn`. Safe because the trace .o was
                // emitted by `lower_trace_into_named` with sig
                // `(I64) -> I64`, matching `TraceFn`. AOT-binary
                // deploy is always 64-bit so the u64 narrows to a
                // valid pointer on this target.
                let fn_ptr_raw = entry.fn_ptr as *const u8;
                let trace_entry: TraceFn = core::mem::transmute::<*const u8, TraceFn>(fn_ptr_raw);
                let ct = CompiledTrace::from_aot_meta(
                    trace_entry,
                    decoded.header.head_pc,
                    decoded.header.n_ops,
                    decoded.header.dispatchable != 0,
                    decoded.header.window_size,
                    entry_tags_rc,
                    exit_tags_rc,
                    tag_res_kind,
                    per_exit_tags_decoded,
                    per_exit_inline_decoded,
                );
                vm.install_aot_trace(proto, ct);
                installed += 1;
                if probe_on {
                    eprintln!(
                        "luna-runtime-helpers: aot_trace_installed head_pc={}",
                        decoded.header.head_pc
                    );
                }
            }
        }
        installed
    }
}
