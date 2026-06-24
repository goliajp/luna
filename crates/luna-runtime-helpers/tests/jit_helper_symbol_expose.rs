//! v1.3 Stage 7 follow-on — smoke test for the helper-symbol expose.
//!
//! `luna-runtime-helpers` is the staticlib that AOT-produced binaries
//! statically link against. When the embedded `.o` (Cranelift-lowered
//! trace mcode) calls `luna_jit_*` helpers (table set/get, op_concat,
//! upval_get, …), the link step needs those 27 symbols to be present
//! as strong externs in the staticlib.
//!
//! This test pins that contract in two layers:
//!
//! 1. **Rust-level reachability** — each of the 27 helpers is
//!    re-exported via `pub use luna_runtime_helpers::luna_jit_*`
//!    (lib.rs). The test takes the address of each one to fail
//!    compilation if the re-export ever drifts.
//!
//! 2. **`force_link_jit_helpers` returns 27** — confirms the
//!    `LUNA_AOT_HELPER_PIN` static is alive and indexes every helper
//!    once. A future PR that adds a 28th `pub unsafe extern "C" fn
//!    luna_jit_*` to `crates/luna-jit/src/jit_backend/mod.rs` will
//!    fail this assertion until the pin array grows.
//!
//! What this DOES NOT cover:
//!
//! - The link-time symbol presence in the produced staticlib. That
//!   contract is enforced in `crates/luna-aot/tests/
//!   stage7_aot_helpers_in_staticlib.rs`, which is the layer that
//!   actually shells out to `nm` against the built `.a`. Putting that
//!   here would create a circular dep (this crate doesn't know about
//!   `luna-aot`'s build pipeline).
//!
//! - End-to-end "AOT binary fires AOT mcode on hot loop". That's the
//!   `iconst`-relocation + trace-dispatch-install workstream, scoped
//!   out per `.dev/rfcs/v1.3-rfc-trace-aot-relocation.md`.

#![cfg(feature = "jit-helpers")]

use luna_runtime_helpers as lrh;

/// The 27 helper symbols, re-checked via the crate's `pub use`
/// re-exports. Compile-fails if any name drifts out of upstream
/// `luna-jit::jit_backend`.
#[test]
fn all_27_helpers_reexported() {
    // Address-of each helper — forces the compiler to resolve every
    // `pub use` re-export at type-check time. If any helper is renamed
    // or removed upstream, this test fails to build with a clear
    // "cannot find function in crate" message.
    let helpers: [*const (); 27] = [
        lrh::luna_jit_new_table as *const (),
        lrh::luna_jit_new_table_sized as *const (),
        lrh::luna_jit_materialize_sunk_table as *const (),
        lrh::luna_jit_table_set_int as *const (),
        lrh::luna_jit_table_set_raw as *const (),
        lrh::luna_jit_table_set_field as *const (),
        lrh::luna_jit_table_get_field as *const (),
        lrh::luna_jit_op_get_tab_up as *const (),
        lrh::luna_jit_table_set_nil as *const (),
        lrh::luna_jit_table_set_float_float as *const (),
        lrh::luna_jit_table_get_int as *const (),
        lrh::luna_jit_table_get_float as *const (),
        lrh::luna_jit_upval_get as *const (),
        lrh::luna_jit_op_close as *const (),
        lrh::luna_jit_stack_update_raw as *const (),
        lrh::luna_jit_op_concat as *const (),
        lrh::luna_jit_str_buf_acquire as *const (),
        lrh::luna_jit_str_buf_release as *const (),
        lrh::luna_jit_str_buf_extend as *const (),
        lrh::luna_jit_str_buf_intern as *const (),
        lrh::luna_jit_op_tforcall as *const (),
        lrh::luna_jit_stack_load as *const (),
        lrh::luna_jit_stack_tag as *const (),
        lrh::luna_jit_spill_to_stack as *const (),
        lrh::luna_jit_op_closure as *const (),
        lrh::luna_jit_trace_materialize_frames as *const (),
        lrh::luna_jit_table_len as *const (),
    ];
    // Sanity-check non-null. `*const ()` for an `extern "C"` symbol
    // never being null is a Rust-side invariant — this is just a
    // belt-and-braces guard against future macro-rewrite mishaps.
    for (i, h) in helpers.iter().enumerate() {
        assert!(
            !h.is_null(),
            "helper #{i} resolved to null — re-export broken?"
        );
    }
}

/// The pin array length must match the helper count. Doubles as the
/// "did we forget to grow the array when adding a 28th helper" gate.
#[test]
fn force_link_jit_helpers_reports_27() {
    let n = lrh::force_link_jit_helpers();
    assert_eq!(
        n, 27,
        "LUNA_AOT_HELPER_PIN must hold all 27 helper symbols; if you \
         added a `pub unsafe extern \"C\" fn luna_jit_*` upstream, \
         grow both `luna-runtime-helpers/src/lib.rs::LUNA_AOT_HELPER_PIN` \
         AND this assertion."
    );
}
