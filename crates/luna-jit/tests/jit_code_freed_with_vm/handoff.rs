//! Levels of compiled code that hand themselves to the interpreter write
//! their frames onto the stack: nothing is allocated while compiled code
//! runs. A policy refuses every block compiled code asks for, so what the
//! counting allocator sees there is what goes around the Vm's context.

use super::*;
use luna_jit::runtime::Value;
use luna_jit::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};

struct RefuseInJit;

impl MemoryPolicy for RefuseInJit {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        luna_jit::jit_backend::__jit_tls_ptrs().0.is_null()
    }
}

/// A function compiled whole that recurses and makes a table at the
/// bottom: refused the table, every level hands its frame over.
const DEEP: &str = "local function down(d) if d == 0 then return {d} end \
       local t = down(d - 1) return t end \
     local s = 0 for i = 1, 40 do s = s + #down(12) end return s";

#[test]
fn levels_handed_over_allocate_nothing() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for v in [
        luna_jit::LuaVersion::Lua53,
        luna_jit::LuaVersion::Lua54,
        luna_jit::LuaVersion::Lua55,
    ] {
        let mut vm = luna_jit::vm::Vm::new_with_mem(v, MemOwner::policy(Box::new(RefuseInJit)));
        luna_jit::install_default_jit(&mut vm);
        vm.set_trace_jit_enabled(false);
        vm.set_call_hot_threshold(1);
        let cl = vm.load(DEEP.as_bytes(), b"=m").expect("load");
        let before = IN_JIT.load(Ordering::Relaxed);
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let n = IN_JIT.load(Ordering::Relaxed) - before;
        assert!(matches!(r[0], Value::Int(40)), "{v:?}: {:?}", r[0]);
        assert!(
            vm.jit().counters.method_compiled > 0,
            "{v:?}: nothing compiled whole"
        );
        assert_eq!(n, 0, "{v:?}: compiled code allocated {n} times");
    }
}
