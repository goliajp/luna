//! P15-A v2-A — side-trace COMPILE + parent ptr LINK. The
//! observability layer v1 wired up the recorder; v2-A lifts the
//! discard short-circuit and lets side traces compile through the
//! normal lowerer path. The compiled side trace is pinned
//! `dispatchable=false` (entered only via parent's exit ptr in
//! v2-B/C, not via the back-edge / call-trigger lookup), and its
//! entry fn ptr is written into the parent's
//! `exit_side_trace_ptrs[parent_exit_idx]` Cell.
//!
//! v2-A is the LINK foundation. v2-B/C will modify the IR at the
//! parent's 22 emit_store_back_and_return_pc sites to read the
//! ptr and indirect-call when non-null.

use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// fib(28) — at least one side trace must compile (counter ≥ 1)
/// AND at least one parent exit_side_trace_ptrs cell must be
/// written (non-null after compile).
#[test]
fn fib_28_compiles_side_trace() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local function f(n)
                  if n < 2 then return n end
                  return f(n-1) + f(n-2)
              end
              return f(28)",
            b"=fib28",
        )
        .unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(317811)));

    let started = vm.trace_side_trace_started_count();
    let compiled = vm.trace_side_trace_compiled_count();
    assert!(
        started >= 1,
        "fib_28 must have STARTED at least one side trace; got {}",
        started
    );
    assert!(
        compiled >= 1,
        "fib_28 must have COMPILED at least one side trace; got \
         started={} compiled={}",
        started,
        compiled
    );
    // v2-A invariant: compiled ≤ started (gap = abort / compile fail).
    assert!(
        compiled <= started,
        "compiled ({}) must be <= started ({})",
        compiled,
        started
    );

    // Parent's exit_side_trace_ptrs[idx] should be non-null for at
    // least one slot. Walk every trace on the fib proto chain via
    // hot_exit_iter — each `head_proto` carries a `CompiledTrace`
    // whose ptrs we can inspect.
    let any_non_null = walk_any_side_ptr_non_null(cl);
    assert!(
        any_non_null,
        "fib_28 must have written at least one parent's \
         exit_side_trace_ptrs cell to non-null; compiled={}",
        compiled
    );
}

/// binary_trees_d4 — counter ≥ 1 + at least one parent ptr written.
#[test]
fn binary_trees_d4_compiles_side_trace() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local function make(d)
                  if d == 0 then return 1 end
                  return 1 + make(d-1) + make(d-1)
              end
              local s = 0
              for i = 1, 200 do s = s + make(4) end
              return s",
            b"=btrees_d4",
        )
        .unwrap();
    let r = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();
    assert!(matches!(r[0], luna_jit::runtime::Value::Int(6200)));

    assert!(
        vm.trace_side_trace_compiled_count() >= 1,
        "binary_trees_d4 must compile at least one side trace; got {}",
        vm.trace_side_trace_compiled_count()
    );
    assert!(
        walk_any_side_ptr_non_null(cl),
        "binary_trees_d4 must write at least one parent exit ptr"
    );
}

/// Negative: concat_str never triggers, so never compiles a side
/// trace. Counter must remain 0.
#[test]
fn concat_str_does_not_compile_side_trace() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let _ = vm
        .eval(
            "local t = {}
             for i = 1, 10000 do t[i] = 'x' end
             local function join(tt)
                 local s = ''
                 for _, v in ipairs(tt) do s = s..v end
                 return s
             end
             return string.len(join(t))",
        )
        .unwrap();
    assert_eq!(
        vm.trace_side_trace_compiled_count(),
        0,
        "concat_str_for_10k must NOT compile any side trace; got {}",
        vm.trace_side_trace_compiled_count()
    );
}

/// Side traces are pinned `dispatchable=false`. The dispatched
/// counter on fib_28 must reflect ONLY primary-trace dispatches —
/// the side trace's `entry` is never invoked via the standard
/// `traces.find(|t| t.head_pc == pc && t.dispatchable)` path. Until
/// v2-B/C wires the indirect call, the primary dispatch count must
/// remain non-zero (interp path still hot).
#[test]
fn side_trace_pinned_non_dispatchable_for_v2_a() {
    let mut vm = luna_jit::new_with_jit(LuaVersion::Lua55);
    vm.set_jit_enabled(false);
    vm.set_trace_jit_enabled(true);

    let cl = vm
        .load(
            b"local function f(n)
                  if n < 2 then return n end
                  return f(n-1) + f(n-2)
              end
              return f(28)",
            b"=fib28",
        )
        .unwrap();
    let _ = vm
        .call_value(luna_jit::runtime::Value::Closure(cl), &[])
        .unwrap();

    // Verify by walking traces: every side-trace-flagged CompiledTrace
    // (we identify those by trace_side_trace_compiled_count > 0 +
    // dispatchable=false existence) must be marked non-dispatchable.
    let (dispatchable_count, non_dispatchable_count) = count_dispatchable_split(cl);
    assert!(
        vm.trace_side_trace_compiled_count() >= 1,
        "fib_28 must compile a side trace for this assertion to mean \
         anything; got {}",
        vm.trace_side_trace_compiled_count()
    );
    assert!(
        non_dispatchable_count >= vm.trace_side_trace_compiled_count() as usize,
        "every compiled side trace must be pinned dispatchable=false; \
         non_dispatchable={} compiled_side_trace={}",
        non_dispatchable_count,
        vm.trace_side_trace_compiled_count()
    );
    // sanity: primary fib trace stays dispatchable, dispatched-count >= 1.
    assert!(
        dispatchable_count >= 1,
        "primary fib trace must remain dispatchable"
    );
    assert!(
        vm.trace_dispatched_count() >= 1,
        "primary fib trace must still dispatch at runtime"
    );
}

/// Walk `cl.proto` recursively, return `true` if any
/// `CompiledTrace.exit_side_trace_ptrs` cell is non-null.
fn walk_any_side_ptr_non_null(
    cl: luna_jit::runtime::heap::Gc<luna_jit::runtime::function::LuaClosure>,
) -> bool {
    fn walk(proto: luna_jit::runtime::heap::Gc<luna_jit::runtime::function::Proto>) -> bool {
        for ct in proto.traces.borrow().iter() {
            for cell in ct.exit_side_trace_ptrs.iter() {
                if !cell.get().is_null() {
                    return true;
                }
            }
        }
        for inner in proto.protos.iter() {
            if walk(*inner) {
                return true;
            }
        }
        false
    }
    walk(cl.proto)
}

/// Return `(dispatchable_count, non_dispatchable_count)` across every
/// trace reachable from `cl.proto`.
fn count_dispatchable_split(
    cl: luna_jit::runtime::heap::Gc<luna_jit::runtime::function::LuaClosure>,
) -> (usize, usize) {
    fn walk(
        proto: luna_jit::runtime::heap::Gc<luna_jit::runtime::function::Proto>,
        d: &mut usize,
        nd: &mut usize,
    ) {
        for ct in proto.traces.borrow().iter() {
            if ct.dispatchable {
                *d += 1;
            } else {
                *nd += 1;
            }
        }
        for inner in proto.protos.iter() {
            walk(*inner, d, nd);
        }
    }
    let mut d = 0;
    let mut nd = 0;
    walk(cl.proto, &mut d, &mut nd);
    (d, nd)
}
