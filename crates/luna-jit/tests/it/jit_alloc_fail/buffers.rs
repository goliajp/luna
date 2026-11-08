//! The buffers compiled code runs with come from the Vm's allocation
//! context: a policy sees them, and when it refuses one the program still
//! computes what it computes without the JIT.

use super::*;

/// A policy refusing, once armed, the new blocks of kind `Other` that
/// `pick` chooses by size, inside compiled code only when `compiled`, up
/// to `limit` of them.
struct RefuseNew {
    armed: Arc<AtomicBool>,
    compiled: bool,
    pick: fn(usize) -> bool,
    limit: usize,
    refused: Arc<AtomicUsize>,
}

impl MemoryPolicy for RefuseNew {
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, _in_use: usize) -> bool {
        let refuse = self.armed.load(Relaxed)
            && (!self.compiled || in_compiled_code())
            && kind == BlockKind::Other
            && old == 0
            && (self.pick)(new)
            && self.refused.load(Relaxed) < self.limit;
        if refuse {
            self.refused.fetch_add(1, Relaxed);
        }
        !refuse
    }
}

/// `src` run with the trace JIT on and the method JIT off under a
/// [`RefuseNew`] armed once the Vm is made: its result, the blocks refused
/// and the traces dispatched.
fn run_refusing(
    v: LuaVersion,
    src: &str,
    compiled: bool,
    pick: fn(usize) -> bool,
    limit: usize,
) -> (String, usize, u64, Vm) {
    let armed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicUsize::new(0));
    let policy = RefuseNew {
        armed: armed.clone(),
        compiled,
        pick,
        limit,
        refused: refused.clone(),
    };
    let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(policy)));
    luna_jit::install_default_jit(&mut vm);
    vm.set_jit_enabled(false);
    vm.set_trace_hot_threshold(1);
    armed.store(true, Relaxed);
    let r = result(&mut vm, src);
    let dispatched = vm.trace_dispatched_count();
    (r, refused.load(Relaxed), dispatched, vm)
}

/// The least a trace's room for what its exits leave unmade takes.
const ROOM: usize = luna_jit::__private::SUNK_WORDS_MAX * std::mem::size_of::<Value>();

/// A trace's room for what its exits leave unmade comes from the context:
/// refused, the trace is not installed, and the interpreter runs the loop.
#[test]
fn a_trace_there_is_no_room_to_run_is_not_installed() {
    let src = "local s = 0 for i = 1, 3000 do s = s + i % 7 end return s";
    for v in VERSIONS {
        let want = interpreted(v, src);
        let (r, refused, dispatched, vm) = run_refusing(v, src, false, |n| n >= ROOM, usize::MAX);
        assert_eq!(r, want, "{v:?}");
        assert!(refused > 0, "{v:?}: the policy saw no trace's room");
        assert_eq!(dispatched, 0, "{v:?}: a trace with no room ran");
        assert_eq!(
            vm.jit().counters.install_no_room as usize,
            refused,
            "{v:?}: a trace left out for want of room not counted"
        );
    }
}

/// Memory runs short for a while: a head whose traces found no room more
/// times than a head is given up after failing to compile is not given
/// up, and its trace runs once there is room.
#[test]
fn a_head_short_of_room_for_a_while_is_not_given_up() {
    let src = "local s = 0 for i = 1, 20000 do s = s + i % 7 end return s";
    for v in VERSIONS {
        let want = interpreted(v, src);
        let (r, refused, dispatched, vm) = run_refusing(v, src, false, |n| n >= ROOM, 64);
        assert_eq!(r, want, "{v:?}");
        assert_eq!(refused, 64, "{v:?}: the head was not recorded again");
        assert_eq!(vm.jit().counters.install_no_room, 64, "{v:?}");
        assert!(dispatched > 0, "{v:?}: the trace never ran");
    }
}

/// The box an accumulator buffer lives in comes from the context: refused,
/// the trace leaves at its first concatenation, and the interpreter makes
/// the string.
#[test]
fn an_accumulator_buffer_the_policy_refuses_leaves_the_string_to_the_interpreter() {
    let src = "local s = '' for i = 1, 3000 do local x = 'x' s = s .. x end return #s";
    for v in VERSIONS {
        let want = interpreted(v, src);
        let (r, refused, dispatched, vm) = run_refusing(
            v,
            src,
            true,
            |n| n == std::mem::size_of::<luna_jit::runtime::mem::LVec<u8>>(),
            usize::MAX,
        );
        assert_eq!(r, want, "{v:?}");
        assert!(refused > 0, "{v:?}: the policy saw no buffer's box");
        assert!(dispatched > 0, "{v:?}: no trace ran");
        assert!(
            vm.jit().pending_err().is_none(),
            "{v:?}: a deopt left parked"
        );
    }
}

/// A function compiled whole that recurses and makes a table at the
/// bottom: refused the table, every level hands its frame over.
const DEEP: &str = "local function down(d) if d == 0 then return {d} end \
       local t = down(d - 1) return t end \
     local s = 0 for i = 1, 40 do s = s + #down(12) end return s";

/// The frames compiled levels hand over go onto the stack: refused every
/// block in compiled code, the levels still hand over, and the
/// interpreter makes the table.
#[test]
fn handed_over_frames_take_no_memory() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        let want = interpreted(v, DEEP);
        let p = Refuse::new(None, usize::MAX, true);
        let mut vm = vm_with(v, Some(p), false);
        let cl = vm.load(DEEP.as_bytes(), b"=m").expect("load");
        vm.call_value(Value::Closure(cl), &[]).expect("run");
        assert!(
            matches!(
                cl.proto.protos[0].jit_state(),
                JitProtoState::Compiled { .. }
            ),
            "{v:?}: not compiled"
        );
        for k in 1..=4 {
            let p = Refuse::new(None, k, false);
            let refused = p.refused.clone();
            let mut vm = vm_with(v, Some(p), false);
            assert_eq!(result(&mut vm, DEEP), want, "{v:?} from #{k}");
            assert!(
                refused.load(Relaxed) > 0,
                "{v:?} from #{k}: nothing refused"
            );
            assert!(
                vm.jit().pending_err().is_none(),
                "{v:?}: a deopt left parked"
            );
        }
    }
}
