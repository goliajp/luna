//! v2.0 Track J sub-step J-D regression — `scoped_jit_vm_rebind`
//! RAII guard + `SendJitModule` field sleeve on `JitHandle` /
//! `TraceHandle`.
//!
//! Three things asserted here:
//!
//! 1. **RAII install + restore** — `enter_jit(&mut vm, None)` installs
//!    `&mut vm` in `JIT_VM`, returns a guard; dropping the guard
//!    restores the prior slot value (null at top level).
//!
//! 2. **Nested rebind** — two `enter_jit` calls with two different
//!    Vms; the inner guard's drop restores the outer Vm's pointer,
//!    not null. The outer guard's drop then restores null.
//!
//! 3. **Sleeve type-system assertion** — `JitHandle.__j_d_module()`
//!    and `TraceHandle.__j_d_module()` both return `&SendJitModule`
//!    by signature; the borrow checker enforces that the underlying
//!    `_module` field is `SendJitModule`, not bare `JITModule`. This
//!    fails to compile if J-D's sleeve is ever reverted.
//!
//! Cross-thread `Send` of the wrappers + cross-thread Vm move + the
//! `feature = "send"` flip are J-E's regression, not J-D's.

use luna_core::version::LuaVersion;
use luna_jit::jit::__SendJitModule_for_j_a_test as SendJitModule;
use luna_jit::jit_backend::{__j_d_tls_ptrs, JitHandle, enter_jit, trace::TraceHandle};

/// Compile-time assertion: `JitHandle::__j_d_module` returns
/// `&SendJitModule`. If the `_module` field ever degrades to bare
/// `JITModule` again the signature stops resolving.
fn _assert_jit_handle_module_is_send_jit_module(h: &JitHandle) -> &SendJitModule {
    h.__j_d_module()
}

/// Same compile-time assertion for `TraceHandle`.
fn _assert_trace_handle_module_is_send_jit_module(h: &TraceHandle) -> &SendJitModule {
    h.__j_d_module()
}

/// (1) Top-level enter_jit: TLS goes from null → vm ptr → null on
/// guard drop. The pre-J-D no-op drop would have left the slot
/// pointing at the just-freed Vm; J-D restores the captured null.
#[test]
fn raii_install_and_restore_top_level() {
    // At test start the JIT_VM / JIT_CL slots are null on this
    // thread (no JIT in flight in the test harness).
    let (vm0, cl0) = __j_d_tls_ptrs();
    assert!(vm0.is_null(), "JIT_VM not null at test entry: {vm0:?}");
    assert!(cl0.is_null(), "JIT_CL not null at test entry: {cl0:?}");

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let vm_addr = &mut vm as *mut _ as usize;

    {
        let _guard = enter_jit(&mut vm, None);
        let (vm1, _cl1) = __j_d_tls_ptrs();
        assert!(!vm1.is_null(), "JIT_VM null inside enter_jit scope");
        assert_eq!(
            vm1 as usize, vm_addr,
            "JIT_VM should point at the &mut vm passed to enter_jit"
        );
        // _guard dropped here — restores prev (null).
    }

    let (vm2, cl2) = __j_d_tls_ptrs();
    assert!(
        vm2.is_null(),
        "JIT_VM should be restored to null after guard drop, got {vm2:?}"
    );
    assert!(
        cl2.is_null(),
        "JIT_CL should be restored to null after guard drop, got {cl2:?}"
    );
}

/// (2) Nested enter_jit: outer enters with vm_a, inner enters with
/// vm_b; inner-drop restores vm_a pointer (not null); outer-drop
/// restores null. Exercises the LIFO capture-on-enter / restore-on-
/// drop discipline.
#[test]
fn raii_nested_rebind_restores_outer() {
    let mut vm_a = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let mut vm_b = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    let vm_a_addr = &mut vm_a as *mut _ as usize;
    let vm_b_addr = &mut vm_b as *mut _ as usize;

    // Sanity: distinct Vms.
    assert_ne!(vm_a_addr, vm_b_addr, "vm_a and vm_b must differ");

    let guard_a = enter_jit(&mut vm_a, None);
    let (after_a, _) = __j_d_tls_ptrs();
    assert_eq!(after_a as usize, vm_a_addr, "outer enter pinned vm_a");

    {
        let guard_b = enter_jit(&mut vm_b, None);
        let (after_b, _) = __j_d_tls_ptrs();
        assert_eq!(after_b as usize, vm_b_addr, "inner enter pinned vm_b");
        drop(guard_b);
    }

    // Inner guard dropped: outer's vm_a must be restored, NOT null,
    // NOT left pointing at the freed-frame vm_b borrow.
    let (after_drop_b, _) = __j_d_tls_ptrs();
    assert_eq!(
        after_drop_b as usize, vm_a_addr,
        "inner guard drop must restore outer vm_a pointer, got {after_drop_b:?}"
    );

    drop(guard_a);
    let (after_drop_a, _) = __j_d_tls_ptrs();
    assert!(
        after_drop_a.is_null(),
        "outer guard drop must restore null, got {after_drop_a:?}"
    );
}

/// (3) Closure pinning: `enter_jit(..., None)` writes null to JIT_CL,
/// drop restores prev. Belt-and-suspenders alongside the JIT_VM
/// assertions above (the restore_fn writes both slots in one shot).
#[test]
fn raii_restores_jit_cl_alongside_jit_vm() {
    let (vm0, cl0) = __j_d_tls_ptrs();
    assert!(vm0.is_null() && cl0.is_null());

    let mut vm = luna_jit::new_minimal_with_jit(LuaVersion::Lua55);
    {
        let _guard = enter_jit(&mut vm, None);
        let (_vm_after, cl_after) = __j_d_tls_ptrs();
        // We passed `None` for the closure, so JIT_CL is null inside
        // the scope — but the restore on drop still must put back
        // whatever was there before (also null in this top-level case).
        assert!(cl_after.is_null(), "None-closure enter sets JIT_CL = null");
    }
    let (vm1, cl1) = __j_d_tls_ptrs();
    assert!(vm1.is_null(), "JIT_VM restored to null");
    assert!(cl1.is_null(), "JIT_CL restored to null");
}
