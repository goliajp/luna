//! luna's names that PUC makes where the program first uses them: off the
//! list until then, and again after PUC's sweep freed one; freed with the
//! heap wherever they are.

use crate::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use crate::version::LuaVersion;
use crate::vm::Vm;
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};

/// The bytes the heap holds.
struct Track(Arc<AtomicIsize>);

impl MemoryPolicy for Track {
    fn allow(&mut self, old: usize, new: usize, _: BlockKind, _: usize) -> bool {
        self.0.fetch_add(new as isize - old as isize, Relaxed);
        true
    }
    fn freed(&mut self, size: usize) {
        self.0.fetch_sub(size as isize, Relaxed);
    }
}

#[test]
fn a_state_dropped_frees_the_names_off_the_list() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        // one state whose names never left the start, one whose
        // "__metatable" collections freed and the program made again
        for src in [
            "",
            "local t = setmetatable({}, {})
             for i = 1, 4 do
               setmetatable(t, {}) local _ = getmetatable(t) collectgarbage()
             end
             collectgarbage()",
        ] {
            let live = Arc::new(AtomicIsize::new(0));
            {
                let mut vm =
                    Vm::new_minimal_with_mem(v, MemOwner::policy(Box::new(Track(live.clone()))));
                vm.open_base().expect("the basic library opens");
                vm.eval(src).expect("the script runs");
            }
            assert_eq!(
                live.load(Relaxed),
                0,
                "{v:?}: every block given back ({src:?})"
            );
        }
    }
}
