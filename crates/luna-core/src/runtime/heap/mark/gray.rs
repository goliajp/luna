//! The gray stack.

use super::*;

/// The gray stack, outside the allocation context: PUC links gray
/// objects through themselves, so marking allocates nothing a host
/// allocator or memory policy could see or refuse.
#[derive(Default)]
pub(crate) struct GrayStack(Vec<*mut GcHeader>);

impl GrayStack {
    pub(in crate::runtime::heap) fn push(&mut self, h: *mut GcHeader) -> Result<(), ()> {
        self.0.try_reserve(1).map_err(drop)?;
        self.0.push(h);
        Ok(())
    }

    pub(in crate::runtime::heap) fn pop(&mut self) -> Option<*mut GcHeader> {
        self.0.pop()
    }

    pub(in crate::runtime::heap) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(in crate::runtime::heap) fn take(&mut self) -> GrayStack {
        std::mem::take(self)
    }
}
