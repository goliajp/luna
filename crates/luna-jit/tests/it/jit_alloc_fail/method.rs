//! A function the method JIT compiles whole that cannot allocate hands its
//! levels over at the op, and the interpreter goes on with that op.

use super::*;

/// A constructor whose last item is a call of the function itself: an
/// open `SETLIST` after an open call, taking the call's one value.
const OPEN_SETLIST: &str = "local function make(d) \
       if d == 0 then return {1, 1} else return {make(d - 1), make(d - 1)} end end \
     local function size(t) if t[1] == 1 then return #t end \
       return #t + size(t[1]) + size(t[2]) end \
     local s = 0 for i = 1, 8 do s = s + size(make(6)) end return s";

/// Appends: running any store again would show.
const APPENDS: &str = "local function fill(t, n) for i = 1, n do t[#t + 1] = i end return t end \
     local s = 0 for i = 1, 60 do s = s + #fill({}, i * 7) end return s";

/// The allocations compiled code makes running `src`, checking the
/// function the chunk defines first is compiled.
fn method_growths(v: LuaVersion, src: &str) -> usize {
    let p = Refuse::new(None, usize::MAX, true);
    let seen = p.seen.clone();
    let mut vm = vm_with(v, Some(p), false);
    let cl = vm.load(src.as_bytes(), b"=m").expect("load");
    vm.call_value(Value::Closure(cl), &[]).expect("run");
    assert!(
        matches!(
            cl.proto.protos[0].jit_state(),
            JitProtoState::Compiled { .. }
        ),
        "{v:?}: not compiled: {src}"
    );
    seen.load(Relaxed)
}

/// Refuses each allocation compiled code makes, alone and from it on: the
/// result is the interpreter's and no deopt is left parked.
fn sweep_method(src: &str) {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let want = interpreted(v, src);
        let n = method_growths(v, src);
        assert!(n > 0, "{v:?}: compiled code allocated nothing: {src}");
        for k in 1..=n {
            for once in [true, false] {
                let mut vm = vm_with(v, Some(Refuse::new(None, k, once)), false);
                assert_eq!(result(&mut vm, src), want, "{v:?} #{k} once {once}: {src}");
                assert!(
                    vm.jit().pending_err().is_none(),
                    "{v:?} #{k}: a deopt left parked"
                );
            }
        }
    }
}

#[test]
fn a_level_handed_over_at_an_open_setlist_stores_the_call_s_value_only() {
    sweep_method(OPEN_SETLIST);
}

#[test]
fn a_level_handed_over_runs_no_store_again() {
    sweep_method(APPENDS);
}
