//! PUC's count: `totalbytes` and what moves it (objects, blocks, strings,
//! what a load's reader counts set aside, the objects a load makes before
//! PUC does). Its fields are this module's alone; the rest of the model and
//! the VM read and move them through the heap's methods defined here.

mod aside;
mod blocks;
mod defer;
mod objects;
mod total;

pub(in crate::runtime::puc) use aside::LoadsAside;
pub(crate) use defer::Deferred;
pub(in crate::runtime) use objects::{FIXED_EXT, PucKind};
pub(in crate::runtime::puc) use total::Count;
pub(crate) use total::{CountingMark, CountingOff, GC_LIMIT_OFFSET};

/// A token that must be handed back (`CountingOff`, a load's span) was
/// dropped without it: reported in the builds that check.
pub(in crate::runtime::puc) fn unpaired(what: &str) {
    if crate::runtime::gc_hooks::DEBUG_CHECKS && !std::thread::panicking() {
        panic!("{what} dropped without its end");
    }
}
