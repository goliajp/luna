//! What the marker does to a string luna holds by PUC's rules.

use super::super::*;
use crate::runtime::puc::records::parse::KEPT;

impl Marker {
    /// A string luna holds and PUC may not (a load's prototype's, luna's
    /// own names): marked, or left white and [`KEPT`] when it is PUC's and
    /// nothing of PUC's marked it.
    pub(crate) fn keep(&mut self, s: Gc<crate::runtime::LuaStr>) {
        let h = s.header();
        // SAFETY: a string luna holds is allocated; only its header is read
        // and written
        unsafe {
            if is_white((*h).flags()) && !(*h).is_uncounted() && (*h).sub & EXTERNAL == 0 {
                (*h).cls |= KEPT;
                return;
            }
        }
        self.mark(s);
    }
}
