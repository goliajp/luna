//! A Vm that loaded a huge chunk gives the room its parser and compiler
//! took back once the load is over, as PUC frees its parser's: the vectors
//! kept for the next load stay at what an ordinary chunk needs.

use std::cell::Cell;
use std::rc::Rc;

use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// The most bytes the Vm had in use at once.
struct Peak(Rc<Cell<usize>>);

impl MemoryPolicy for Peak {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, in_use: usize) -> bool {
        let now = in_use - old.min(in_use) + new;
        self.0.set(self.0.get().max(now));
        true
    }
}

const MIB: usize = 1 << 20;

#[test]
fn a_huge_load_leaves_no_room_held() {
    // 2^16 statements of two expressions each, far past what an ordinary
    // chunk keeps
    let src = "local f = (loadstring or load)(string.rep('x = y\\n', 65536))\n\
               assert(f) f = nil collectgarbage() collectgarbage()";
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let peak = Rc::new(Cell::new(0));
        let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(Peak(peak.clone()))));
        vm.eval("collectgarbage() collectgarbage()").unwrap();
        let before = vm.memory_in_use().unwrap();
        vm.eval(src).unwrap();
        let after = vm.memory_in_use().unwrap();
        assert!(
            peak.get() > before + 16 * MIB,
            "{v:?}: the load took only {} bytes",
            peak.get() - before
        );
        assert!(
            after < before + MIB,
            "{v:?}: {} bytes still in use after the load",
            after - before
        );
    }
}
