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
        let (text, kind) = match tok.tok {
            T::Name(()) => (ScanText::Sym(sym), ScanKind::Name),
            T::Str(()) => (ScanText::Sym(sym), ScanKind::Str),
            ref t => match reserved_index(t) {
                Some(i) => (ScanText::Reserved(i, span), ScanKind::Reserved),
                None => return,
            },
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

/// The reserved word `t`'s index in PUC's `luaX_tokens`, or none.
#[inline(always)]
fn reserved_index<A, B, C>(t: &crate::frontend::token::Token<A, B, C>) -> Option<u8> {
    use crate::frontend::token::Token as T;
    Some(match t {
        T::And => 0,
        T::Break => 1,
        T::Do => 2,
        T::Else => 3,
        T::Elseif => 4,
        T::End => 5,
        T::False => 6,
        T::For => 7,
        T::Function => 8,
        T::Global => 9,
        T::Goto => 10,
        T::If => 11,
        T::In => 12,
        T::Local => 13,
        T::Nil => 14,
        T::Not => 15,
        T::Or => 16,
        T::Repeat => 17,
        T::Return => 18,
        T::Then => 19,
        T::True => 20,
        T::Until => 21,
        T::While => 22,
        _ => return None,
    })
}
