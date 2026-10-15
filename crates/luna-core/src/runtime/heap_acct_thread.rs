//! What PUC allocates for a thread: its state, its stack and its
//! `CallInfo`s.

use super::*;
use crate::runtime::table::Dialect;

impl Heap {
    /// The main thread's calls are `depth` deep: 5.2+ allocate a CallInfo
    /// for each level never reached before (PUC `luaE_extendCI`).
    pub(crate) fn acct_call_depth(&mut self, depth: usize) {
        if self.table_dialect != Dialect::L51 {
            while self.puc.ci_count < depth {
                let ci = self.sizes.callinfo;
                self.acct_block(0, ci);
                self.puc.ci_count += 1;
            }
        }
        self.puc.depth = depth;
    }
}
