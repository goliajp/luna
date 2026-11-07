//! A side trace that cannot allocate leaves at the op, as its parent
//! would: the interpreter does the op, and nothing after the dispatch
//! takes the side trace's refusal for its own.

use super::*;

/// A loop whose trace runs past `i = 100` through a side trace appending
/// tables, the last pass among them; nothing dispatches after it.
const SIDE_LAST: &str = "local t, s = {}, 0 \
     for i = 1, 3000 do s = s + i if i > 100 then t[#t + 1] = {i} end end \
     return #t * 100000000 + s";

/// [`SIDE_LAST`] followed by the first call of a function, which appends
/// within the room its table has (making nothing): run twice, it would
/// show. `jit_on`, when there is one, turns the method JIT on there, so
/// the call compiles the function whole and runs its code (on before the
/// loop, the method JIT would compile the chunk).
const SIDE_THEN_CALL: &str = "local function add(c, i) c[#c + 1] = i return #c end \
     local c = {1, 2, 3, 4} for i = 1, 4 do c[i] = nil end \
     local t, s = {}, 0 \
     for i = 1, 3000 do s = s + i if i > 100 then t[#t + 1] = {i} end end \
     if jit_on then jit_on() end \
     return add(c, 1000) * 1000000000 + #t * 100000 + s % 100000";

fn jit_on(vm: &mut Vm, _: u32, _: u32) -> Result<u32, luna_jit::vm::LuaError> {
    vm.set_jit_enabled(true);
    Ok(0)
}

/// `src` run refusing the tables compiled code makes from the `k`th on
/// (or the `k`th only), with the method JIT off until `jit_on`: the
/// result, the side trace runs, whether no deopt is left parked, whether
/// the function `src` defines first was compiled whole, and the refusals
/// the policy saw a chance for.
fn side_run(v: LuaVersion, src: &str, k: usize, once: bool) -> (String, u64, bool, bool, usize) {
    let p = Refuse::new(Some(BlockKind::Table), k, once);
    let seen = p.seen.clone();
    let mut vm = vm_with(v, Some(p), true);
    vm.set_jit_enabled(false);
    let f = vm.native_with(jit_on, Box::new([]));
    vm.set_global("jit_on", f).expect("jit_on");
    let cl = vm.load(src.as_bytes(), b"=m").expect("load");
    let r = match vm.call_value(Value::Closure(cl), &[]) {
        Ok(r) => match r.first() {
            Some(Value::Int(i)) => i.to_string(),
            Some(Value::Float(f)) => f.to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    let compiled = (cl.proto.protos.first())
        .is_some_and(|p| matches!(p.jit_state(), JitProtoState::Compiled { .. }));
    (
        r,
        vm.trace_side_trace_run_count(),
        vm.jit().pending_err.is_none(),
        compiled,
        seen.load(Relaxed),
    )
}

/// Refuses the tables compiled code makes running `src`, each alone and
/// from each on: the result is the interpreter's, and when the dispatch
/// of the last pass is over no deopt is left parked. `call`: the function
/// `src` defines is compiled whole.
fn sweep_side(v: LuaVersion, src: &str, call: bool) {
    let want = interpreted(v, src);
    let (r, runs, clear, compiled, n) = side_run(v, src, usize::MAX, true);
    assert_eq!(r, want, "{v:?}");
    assert!(clear, "{v:?}: a deopt left parked");
    assert!(runs > 0, "{v:?}: no side trace ran");
    assert_eq!(compiled, call, "{v:?}: the function compiled: {compiled}");
    for k in spread(n, 12) {
        for once in [true, false] {
            let (r, _, clear, _, _) = side_run(v, src, k, once);
            assert_eq!(r, want, "{v:?} #{k} once {once}");
            assert!(clear, "{v:?} #{k} once {once}: a deopt left parked");
        }
    }
}

#[test]
fn a_side_trace_that_cannot_allocate_leaves_no_deopt_parked() {
    for v in VERSIONS {
        sweep_side(v, SIDE_LAST, false);
    }
}

/// The method JIT's call after the loop does not take the side trace's
/// refusal for a deopt of its own and run again.
#[test]
fn a_call_after_a_side_trace_that_cannot_allocate_runs_once() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        sweep_side(v, SIDE_THEN_CALL, true);
    }
}
