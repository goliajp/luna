//! What the opening of the standard libraries made that the model keeps:
//! the functions PUC makes light, and whether 5.1's io environments are
//! on the stack.

use super::super::*;

/// What the opening of the standard libraries made.
#[derive(Default)]
pub(in crate::runtime::puc) struct Opening {
    /// 5.1's io library opening: records of its environments are on the
    /// stack (see `vm/opening/fenv51.rs`)
    phantoms: bool,
    /// the library functions made as PUC's light C functions, by their
    /// number in the opening's tables: one value however often pushed
    lights: Vec<Option<Gc<crate::runtime::NativeClosure>>>,
}

impl Heap {
    /// The light function the opening made as function `id`.
    pub(crate) fn light(&self, id: usize) -> Option<Gc<crate::runtime::NativeClosure>> {
        self.puc.opening.lights.get(id).copied().flatten()
    }

    /// Function `id` of the opening's `count` is the light function `g`:
    /// one value however often pushed.
    pub(crate) fn set_light(
        &mut self,
        id: usize,
        count: usize,
        g: Gc<crate::runtime::NativeClosure>,
    ) {
        let l = &mut self.puc.opening.lights;
        if l.len() <= id {
            l.resize(count.max(id + 1), None);
        }
        l[id] = Some(g);
    }

    /// 5.1's io library opening: its environments' records are on the
    /// stack (see `vm/opening/fenv51.rs`).
    #[inline]
    pub(crate) fn phantoms(&self) -> bool {
        self.puc.opening.phantoms
    }

    #[inline]
    pub(crate) fn set_phantoms(&mut self, on: bool) {
        self.puc.opening.phantoms = on;
    }
}
