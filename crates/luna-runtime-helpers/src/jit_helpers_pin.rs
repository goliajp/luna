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
/// The number of entries (41) must match the number of
/// `pub unsafe extern "C" fn luna_jit_*` in
/// `crates/luna-jit/src/jit_backend/mod.rs`. If luna-jit ever
/// adds a 42nd helper, this array must grow in lock-step
/// or AOT trace `.o`s referencing the new symbol will fail to
/// link with `undefined reference to luna_jit_<new>`.
#[used]
#[unsafe(no_mangle)]
static LUNA_AOT_HELPER_PIN: [PinnedFn; 41] = [
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
    PinnedFn(jb::luna_jit_table_get_int_checked as AnyFn),
    PinnedFn(jb::luna_jit_table_get_field_checked as AnyFn),
    PinnedFn(jb::luna_jit_op_get_tab_up_checked as AnyFn),
    PinnedFn(jb::luna_jit_upval_get_float as AnyFn),
    PinnedFn(jb::luna_jit_self_upval_check as AnyFn),
    PinnedFn(jb::luna_jit_math_fn_is_library as AnyFn),
    PinnedFn(jb::luna_jit_str_sub as AnyFn),
    PinnedFn(jb::luna_jit_park_deopt as AnyFn),
    PinnedFn(jb::luna_jit_suppress_trace_admit as AnyFn),
    PinnedFn(jb::luna_jit_table_set_checked as AnyFn),
    PinnedFn(jb::luna_jit_table_set_int_checked as AnyFn),
    PinnedFn(jb::luna_jit_table_set_field_checked as AnyFn),
    PinnedFn(jb::luna_jit_table_len_checked as AnyFn),
    PinnedFn(jb::luna_jit_head_closure as AnyFn),
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
            let _ = jb::luna_jit_table_get_int_checked(0, 0, 0, std::ptr::null_mut());
            let _ = jb::luna_jit_table_get_field_checked(0, 0, 0, std::ptr::null_mut());
            let _ = jb::luna_jit_op_get_tab_up_checked(0, 0, 0, std::ptr::null_mut());
            let _ = jb::luna_jit_upval_get_float(0);
            let _ = jb::luna_jit_self_upval_check(0);
            let _ = jb::luna_jit_math_fn_is_library(0, 0);
            let _ = jb::luna_jit_str_sub(0, 0, 0);
            jb::luna_jit_park_deopt();
            jb::luna_jit_suppress_trace_admit();
            let _ = jb::luna_jit_table_set_checked(0, 0, 0, 0, 0);
            let _ = jb::luna_jit_table_set_int_checked(0, 0, 0, 0);
            let _ = jb::luna_jit_table_set_field_checked(0, 0, 0, 0);
            let _ = jb::luna_jit_table_len_checked(0);
            let _ = jb::luna_jit_head_closure();
        }
    }

    std::hint::black_box(sum);
    LUNA_AOT_HELPER_PIN.len()
}
