//! 5.1 and 5.2's buffer for concatenations (`G(L)->buff`).

use super::*;

impl Heap {
    /// 5.1 and 5.2's buffer for concatenations made room for `n` bytes
    /// (`luaZ_openspace`, at least `LUA_MINBUFFER`).
    pub(crate) fn open_gbuff(&mut self, n: usize) {
        if n > self.puc.gbuff {
            let size = n.max(32);
            self.acct_block(self.puc.gbuff, size);
            self.puc.gbuff = size;
        }
    }
}
