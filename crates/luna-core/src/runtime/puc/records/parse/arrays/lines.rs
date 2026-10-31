//! The line info of the code array as 5.4+ saves it (`savelineinfo`):
//! relative entries, and an absolute one for a line far from the last.

use super::*;

impl FuncArrays {
    /// The last line saved taken back (5.4+ `removelastlineinfo`): an
    /// absolute entry leaves the line where it was and makes the next one
    /// absolute.
    pub(in crate::runtime::puc) fn unsave_line(&mut self) {
        if self.last_abs {
            self.abs.n -= 1;
            self.iwthabs = 129;
        } else {
            self.prev_line = self.last_prev;
            self.iwthabs = self.iwthabs.saturating_sub(1);
        }
    }
}

impl Heap {
    /// 5.4+ `savelineinfo` without its line info entry: an absolute entry
    /// when the line is 128 or more away from the last one's, or every 128
    /// relative ones.
    #[inline(always)]
    pub(in crate::runtime::puc) fn save_line(&mut self, f: &mut FuncArrays, line: u32) {
        let dif = i64::from(line) - i64::from(f.prev_line);
        let wthabs = f.iwthabs;
        f.iwthabs += 1;
        f.last_prev = f.prev_line;
        f.last_abs = dif.abs() >= 0x80 || wthabs >= 128;
        if f.last_abs {
            self.parser_push(&mut f.abs, ParserArray::AbsLines);
            f.iwthabs = 1;
        }
        f.prev_line = line;
    }
}
