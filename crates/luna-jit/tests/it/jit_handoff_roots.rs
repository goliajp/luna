//! The levels compiled self recursion hands to the interpreter carry the
//! values its registers held, which nothing else holds until they are on
//! the interpreter's stack. Growing the stack for them may collect in an
//! emergency (an allocation refused once), and that collection must find
//! them.

use std::cell::Cell;
use std::rc::Rc;

use luna_core::__private::VmJitState;
use luna_jit::jit_backend::__jit_tls_ptrs;
use luna_jit::runtime::Value;
use luna_jit::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_jit::version::LuaVersion;
use luna_jit::vm::Vm;

/// Refuses, once, the `from`th large growth the interpreter makes (not
/// compiled code): the stack's, among others.
struct RefuseOutside {
    from: usize,
    seen: Rc<Cell<usize>>,
}

impl MemoryPolicy for RefuseOutside {
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, _in_use: usize) -> bool {
        let outside = __jit_tls_ptrs().0.is_null();
        if !outside || kind != BlockKind::Other || new <= old || new < 4096 {
            return true;
        }
        let n = self.seen.get() + 1;
        self.seen.set(n);
        n != self.from
    }
}

/// Each level's `t`, made by the level above, is in no register but its
/// own while the levels below run.
const WALK: &str = "
local function walk(t, n)
  if n == 0 then return 0 end
  local u = {}
  local r = walk(u, n - 1)
  return r + #t
end
local s = 0
for _ = 1, 3 do s = s + walk({0}, 2500) end
return s";

fn run(from: usize) -> (String, usize, usize) {
    let seen = Rc::new(Cell::new(0));
    let policy = RefuseOutside {
        from,
        seen: seen.clone(),
    };
    let mut vm = Vm::new_with_mem(LuaVersion::Lua54, MemOwner::policy(Box::new(policy)));
    luna_jit::install_default_jit(&mut vm);
    vm.set_trace_jit_enabled(false);
    vm.jit_mut().call_hot_threshold = 1;
    let out = match vm.eval(WALK) {
        Ok(r) => match r.first() {
            Some(Value::Int(i)) => i.to_string(),
            other => format!("{other:?}"),
        },
        Err(e) => format!("error: {}", vm.error_text(&e)),
    };
    (out, seen.get(), luna_jit::jit::cache_entry_count(&vm))
}

#[test]
fn an_emergency_collection_while_levels_are_handed_over_keeps_their_values() {
    let (want, growths, compiled) = run(usize::MAX);
    assert_eq!(want, "3");
    assert!(compiled > 0, "nothing compiled");
    assert!(growths > 0, "no growth to refuse");
    for from in 1..=growths {
        let (got, _, _) = run(from);
        assert_eq!(got, want, "the growth refused: {from} of {growths}");
    }
}
