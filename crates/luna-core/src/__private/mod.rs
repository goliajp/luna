//! luna's own crates (the JIT, its helpers, the C API, the CLI, the
//! tools) and its tests reach the runtime's internals through here.
//!
//! **Not part of the public API.** Nothing here is covered by semver: any
//! item may change or go in any release, minor or patch. An embedder
//! uses the documented interface instead; everything an embedder needs
//! is there, without `unsafe`. The functions here that can break memory
//! safety or the collector's invariants when misused are `unsafe fn`,
//! with their conditions under `# Safety`.

mod capi_debug;
mod capi_load;
mod capi_state;
mod capi_values;
mod fields;
mod free;
mod panic_watch;
mod runtime;
mod types;
mod vm_alloc;
mod vm_host;
mod vm_jit;

pub use capi_debug::VmCApiDebug;
pub use capi_load::VmCApiLoad;
pub use capi_state::VmCApiState;
pub use capi_values::VmCApiValues;
pub use fields::{CoroHostInternals, ProtoTraceInternals, TRACE_HEADS_NONE, VmJitState};
pub use free::*;
pub use panic_watch::{panic_free, panics_in};
pub use runtime::*;
pub use types::*;
pub use vm_alloc::VmJitAlloc;
pub use vm_host::VmHostInternals;
pub use vm_jit::VmJitFrames;

/// Have every collection on this thread end by checking the trace
/// being recorded, if any, holds only objects the collector keeps.
#[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
pub fn check_recordings_after_collections(on: bool) {
    crate::vm::Vm::check_recordings_after_collections(on)
}

/// The `gc-stress` controls of a [`Vm`](crate::vm::Vm) luna's tests use.
#[cfg(feature = "gc-stress")]
pub trait VmGcStress {
    /// Collect at checkpoints `offset`, `offset + every`, … from the next
    /// one on, instead of at each.
    fn gc_stress_interval(&mut self, every: u64, offset: u64);
    /// Start keeping the numbers of the checkpoints that collect.
    fn gc_stress_log(&mut self);
    /// The numbers kept.
    fn gc_stress_collected(&self) -> &[u64];
}

#[cfg(feature = "gc-stress")]
impl VmGcStress for crate::vm::Vm {
    fn gc_stress_interval(&mut self, every: u64, offset: u64) {
        crate::vm::Vm::gc_stress_interval(self, every, offset);
    }
    fn gc_stress_log(&mut self) {
        crate::vm::Vm::gc_stress_log(self);
    }
    fn gc_stress_collected(&self) -> &[u64] {
        crate::vm::Vm::gc_stress_collected(self)
    }
}
