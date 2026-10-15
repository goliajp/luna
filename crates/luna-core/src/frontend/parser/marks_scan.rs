//! What a recording parse notes of each token for the load's count: how
//! full PUC's scanner's buffer gets, and the strings it makes.

use super::*;

impl Recording {
    /// Token `ix`, `tok`: how full the scanner's buffer gets reading it,
    /// and the string PUC's scanner makes for it. Up to 5.2 each function
    /// keys the strings in its own table, so every one is kept; later the
    /// chunk's one table takes each string once, so only the first of each.
    #[inline]
    pub(super) fn scan(&mut self, version: LuaVersion, src: &[u8], ix: u32, tok: LexTok, sym: Sym) {
        use crate::frontend::token::Token as T;
        let span = tok.span;
        let (start, end) = (span.start as usize, span.end as usize);
        if self.ends {
            self.scanned.ends.push(span.end);
        }
        // the scanner holds no more than the gap's bytes or the token's
        // and a terminating zero
        if start > self.last_end + 32 || end - start >= 32 {
            self.peak(version, src, ix, span);
        }
        self.last_end = end;
        let reserved = |i: u8| (ScanText::Reserved(i, span), ScanKind::Reserved);
        let (text, kind) = match tok.tok {
            T::Name(()) => (ScanText::Sym(sym), ScanKind::Name),
            T::Str(()) => (ScanText::Sym(sym), ScanKind::Str),
            T::And => reserved(0),
            T::Break => reserved(1),
            T::Do => reserved(2),
            T::Else => reserved(3),
            T::Elseif => reserved(4),
            T::End => reserved(5),
            T::False => reserved(6),
            T::For => reserved(7),
            T::Function => reserved(8),
            T::Global => reserved(9),
            T::Goto => reserved(10),
            T::If => reserved(11),
            T::In => reserved(12),
            T::Local => reserved(13),
            T::Nil => reserved(14),
            T::Not => reserved(15),
            T::Or => reserved(16),
            T::Repeat => reserved(17),
            T::Return => reserved(18),
            T::Then => reserved(19),
            T::True => reserved(20),
            T::Until => reserved(21),
            T::While => reserved(22),
            _ => return,
        };
        if version >= LuaVersion::Lua53 {
            // names are numbered as they first appear
            match text {
                ScanText::Sym(s) if s.0 < self.next_sym => return,
                ScanText::Sym(s) => self.next_sym = s.0 + 1,
                ScanText::Reserved(i, _) if self.reserved_seen & 1 << i != 0 => return,
                ScanText::Reserved(i, _) => self.reserved_seen |= 1 << i,
            }
        }
        self.scanned.strings.push((ix, text, kind));
    }

    #[cold]
    #[inline(never)]
    fn peak(&mut self, version: LuaVersion, src: &[u8], ix: u32, span: Span) {
        let gap = &src[self.last_end.min(span.start as usize)..span.start as usize];
        let peak = scan_buf::peak(version, gap, span.slice(src));
        if peak > 32 {
            self.scanned.peaks.push((ix, peak as u32));
        }
    }
}
