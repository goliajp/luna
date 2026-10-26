//! The minimal state every constructor starts from.

use super::*;
use crate::runtime::puc::api::{PucCollector, PucCount};

impl Vm {
    pub(super) fn new_minimal_on(version: LuaVersion, heap: Heap) -> Vm {
        Vm::try_new_minimal_on(version, heap)
            .unwrap_or_else(|| crate::runtime::mem::oom_abort(std::alloc::Layout::new::<Vm>()))
    }

    pub(super) fn try_new_minimal_on(version: LuaVersion, mut heap: Heap) -> Option<Vm> {
        heap.set_version(version).ok()?;
        // luna's own objects PUC made at other points, or not at all
        let off = heap.counting_off();
        let Some(mut vm) = Vm::build_minimal(version, heap) else {
            // the heap went with the state that failed
            off.abandon();
            return None;
        };
        vm.heap.counting_on(off);
        vm.heap.refresh_gc_limit();
        Some(vm)
    }

    /// The state [`Vm::try_new_minimal_on`] makes, with counting off.
    fn build_minimal(version: LuaVersion, heap: Heap) -> Option<Vm> {
        let mut vm = Vm::new_inner(version, heap)?;
        let mc = vm.heap.try_new_coro(Value::Nil, vm.globals).ok()?;
        vm.heap.main_thread_made(mc);
        vm.main_coro = Some(mc);
        // PUC's `init_registry`, counted with the state
        vm.host_make_registry().ok()?;
        let (a, b) = vm.rng_auto_seed();
        vm.rng_seed(a as u64, b as u64);
        vm.buf_spare
            .reserve_exact(Vm::lual_buffersize(version))
            .ok()?;
        vm.state_complete = true;
        Some(vm)
    }
}
