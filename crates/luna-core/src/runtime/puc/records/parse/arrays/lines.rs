//! The line info of the code array as 5.4+ saves it (`savelineinfo`):
//! relative entries, and an absolute one for a line far from the last.

use super::*;

impl FuncArrays {
    /// The last line saved taken back (5.4+ `removelastlineinfo`): an
    /// absolute entry leaves the line where it was and makes the next one
    /// absolute.
    fn unsave_line(&mut self) {
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
    /// One instruction coded on `line` (`luaK_code`): the code array, then
    /// the line info (5.4+ `savelineinfo`, with its absolute entries).
    #[inline(always)]
    pub(in crate::runtime) fn parser_code(&mut self, f: &mut FuncArrays, line: u32) {
        self.parser_push(&mut f.code, ParserArray::Code);
        if matches!(self.puc.dialect, Dialect::L54 | Dialect::L55) {
            self.save_line(f, line);
        }
        self.parser_push(&mut f.line, ParserArray::LineInfo);
    }

    /// The last instruction moved to `line` (5.4+ `luaK_fixline`: its line
    /// taken back and saved again).
    pub(in crate::runtime) fn parser_fix(&mut self, f: &mut FuncArrays, line: u32) {
        if matches!(self.puc.dialect, Dialect::L54 | Dialect::L55) {
            f.unsave_line();
            self.save_line(f, line);
        }
    }

    /// 5.4+ `savelineinfo` without its line info entry: an absolute entry
    /// when the line is 128 or more away from the last one's, or every 128
    /// relative ones.
    #[inline(always)]
    fn save_line(&mut self, f: &mut FuncArrays, line: u32) {
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

    /// The last instruction taken back, with its line info (5.4+
    /// `removelastlineinfo`).
    pub(in crate::runtime) fn parser_uncode(&mut self, f: &mut FuncArrays) {
        f.code.n -= 1;
        f.line.n -= 1;
        f.unsave_line();
    }
}
