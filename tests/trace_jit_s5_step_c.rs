//! P12-S5-C — materialise-on-deopt at cmp side-exits.
//!
//! Removes the S5-B `body_has_cmp` gate. The escape sweep no longer
//! auto-escapes live sunk sites at an `Op::Lt`/`Op::Le`/`Op::Eq`/
//! `Op::EqK`. Emit instead inserts a `luna_jit_materialize_sunk_table`
//! call at every depth=0 cmp side-exit for each live Sinkable site:
//! stack-allocates a `cap × i64` raws buffer + `cap × u8` kinds
//! buffer, fills them from virt slots + `virt_kinds`, calls the
//! helper, `def_var`s the returned heap-table bits into
//! `regs_full[site.a]`, and overrides the per-exit-tags snapshot
//! entry for the slot to `RegKind::Table` so the dispatcher
//! repacks correctly on deopt.
//!
//! These tests verify:
//! 1. Side-exit fires on a fraction of iterations → interp resume
//!    reads the materialised heap table correctly (result matches
//!    interpreter / heap-path semantics).
//! 2. Both `if/else` branches of a cmp can deopt: emit must
//!    materialise on BOTH side-exit edges of the cmp+Jmp pair —
//!    which it does because the recorder only ever sees one
//!    direction, and the side-exit handles the other.
//! 3. Inline-cmp gate: a recursive function with a body cmp at
//!    inline_depth>0 still demotes (S5-C v1 only materialises at
//!    depth=0 cmps).

use luna::version::LuaVersion;
use luna::vm::Vm;

/// `for i = 1, 1000 do local t = {1, 2, 3}; if i > 500 then s = s + t[1] end end`.
/// The recorder fires at the back-edge after ~64 iters where
/// `i > 500` is false → body emits the "cond false" path. Once
/// `i > 500` becomes true (iter 501), the cmp side-exit fires;
/// interp resumes at the `then` branch and reads `t[1]`. Without
/// the materialise-on-deopt path, `t` would be Nil and the
/// resume would error. Materialise = correct result 500.
#[test]
fn cmp_side_exit_materialises_for_interp_resume() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {1, 2, 3}
                 if i > 500 then s = s + t[1] end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(500)),
        "expected Int(500); a wrong/Nil t[1] read on deopt would \
         either error or sum incorrectly. got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "site stays Sinkable under S5-C; got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
    assert!(
        vm.trace_materialize_emit_count() >= 1,
        "the cmp side-exit must emit the materialise helper; \
         got materialize_emit_count={}",
        vm.trace_materialize_emit_count()
    );
}

/// `for i = 1, 1000 do local t = {10, 20}; if i % 2 == 0 then s = s + t[1] else s = s + t[2] end end`.
/// Both branches access `t`. The recorder picks one direction;
/// the other becomes a side-exit. interp resume needs the
/// materialised table for either branch. Sum = 500 × 10 + 500 × 20 = 15000.
#[test]
fn cmp_side_exit_both_branches_access_table() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local s = 0
             for i = 1, 1000 do
                 local t = {10, 20}
                 if i % 2 == 0 then s = s + t[1] else s = s + t[2] end
             end
             return s",
        )
        .unwrap();
    assert!(
        matches!(r[0], luna::runtime::Value::Int(15000)),
        "expected Int(15000); a wrong t[1]/t[2] on deopt would \
         skew. got {:?}",
        r[0]
    );
    assert!(
        vm.trace_sunk_alloc_count() >= 1,
        "site must take sunk emit with the cmp present; \
         got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
}

/// Self-recursive `f(n)` with a NewTable in its body. The base-case
/// cmp `if n < 2 then return n end` is at inline_depth>0 during
/// step4b inlining. The `has_inline_cmp` pre-emit gate demotes any
/// Sinkable site since v1's materialise emit only covers depth=0
/// cmps. Heap path stays; result still correct.
#[test]
fn inline_cmp_demotes_sunk_site() {
    let mut vm = Vm::new(LuaVersion::Lua54);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let r = vm
        .eval(
            "local function f(n)
                 local t = {n, n + 1}
                 if n < 2 then return t[1] + t[2] end
                 return t[1] + f(n - 1)
             end
             return f(5)",
        )
        .unwrap();
    // f(5)=5+f(4)=5+4+f(3)=5+4+3+f(2)=5+4+3+2+f(1)=5+4+3+2+(1+2)=17.
    // n=1 base case: t={1,2}; t[1]+t[2] = 3.
    // Recursion: 5+4+3+2+3 = 17.
    assert!(
        matches!(r[0], luna::runtime::Value::Int(17)),
        "expected Int(17), got {:?}",
        r[0]
    );
    assert_eq!(
        vm.trace_sunk_alloc_count(),
        0,
        "inline-cmp gate must demote sunk sites; got sunk_alloc_count={}",
        vm.trace_sunk_alloc_count()
    );
}
