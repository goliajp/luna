//! Identifier interning for the load path (PUC `luaX_newstring` keeps one
//! string per distinct name): the lexer hands the parser a number per
//! identifier, the parser builds the AST with them and the compiler
//! compares numbers. The text lives once, in one buffer; the public
//! token and AST types, which own their names, are produced from it only
//! where they are handed out.

/// An interned identifier: an index into [`Names`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) struct Sym(pub(crate) u32);

/// An identifier in the internal AST: its number and the line it was read
/// on (the internal counterpart of [`crate::frontend::ast::Name`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct SymName {
    pub(crate) sym: Sym,
    pub(crate) line: u32,
}

/// The identifiers of one chunk.
#[derive(Default)]
pub(crate) struct Names {
    /// every name back to back
    text: String,
    /// where each name starts in `text`; the last entry is the end
    ends: Vec<u32>,
    /// open addressing over `ends` indices plus one (0 is empty); its
    /// length is a power of two, at most half full
    table: Vec<u32>,
}

impl Names {
    pub(crate) fn with_capacity(src_len: usize) -> Names {
        // about one distinct identifier per 32 source bytes
        let n = (src_len / 32).max(8);
        let mut ends = Vec::with_capacity(n + 1);
        ends.push(0);
        Names {
            text: String::with_capacity(n * 6),
            ends,
            table: vec![0; (2 * n).next_power_of_two()],
        }
    }

    fn hash(s: &[u8]) -> u32 {
        // FNV-1a: identifiers are short
        let mut h: u32 = 0x811c_9dc5;
        for &b in s {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        h
    }

    /// The number of `s`, giving it one when it is new.
    pub(crate) fn intern(&mut self, s: &str) -> Sym {
        if self.ends.is_empty() {
            *self = Names::with_capacity(0);
        }
        let mask = self.table.len() - 1;
        let mut i = Self::hash(s.as_bytes()) as usize & mask;
        loop {
            match self.table[i] {
                0 => break,
                e => {
                    let sym = Sym(e - 1);
                    if self.text(sym) == s {
                        return sym;
                    }
                }
            }
            i = (i + 1) & mask;
        }
        let sym = Sym(self.ends.len() as u32 - 1);
        self.text.push_str(s);
        self.ends.push(self.text.len() as u32);
        self.table[i] = sym.0 + 1;
        if (self.ends.len() - 1) * 2 > self.table.len() {
            self.grow();
        }
        sym
    }

    fn grow(&mut self) {
        let len = self.table.len() * 2;
        let mut table = vec![0; len];
        for s in 0..self.ends.len() as u32 - 1 {
            let mut i = Self::hash(self.text(Sym(s)).as_bytes()) as usize & (len - 1);
            while table[i] != 0 {
                i = (i + 1) & (len - 1);
            }
            table[i] = s + 1;
        }
        self.table = table;
    }

    /// The text of a name.
    pub(crate) fn text(&self, s: Sym) -> &str {
        let i = s.0 as usize;
        &self.text[self.ends[i] as usize..self.ends[i + 1] as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_same_number() {
        let mut n = Names::with_capacity(0);
        let words: Vec<String> = (0..200).map(|i| format!("v{}", i % 70)).collect();
        let syms: Vec<Sym> = words.iter().map(|w| n.intern(w)).collect();
        for (w, s) in words.iter().zip(&syms) {
            assert_eq!(n.text(*s), w);
            assert_eq!(n.intern(w), *s);
        }
        assert_eq!(n.ends.len() - 1, 70);
        assert_ne!(n.intern("a"), n.intern("b"));
        let e = n.intern("");
        assert_eq!(n.text(e), "");
    }
}
