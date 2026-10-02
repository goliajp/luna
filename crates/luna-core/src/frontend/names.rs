//! The identifiers and string literals of a chunk, each kept once (PUC
//! `luaX_newstring` keeps one string per distinct name): the lexer hands
//! the parser a number per identifier or literal, the tree stores the
//! numbers and the compiler compares them. The bytes live back to back in
//! one buffer owned by the chunk.

/// An interned identifier or string literal: an index into [`Names`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub struct Sym(
    /// Zero-based number of the entry, in order of first appearance.
    pub u32,
);

/// The identifiers and string literals of one chunk.
#[derive(Clone, Debug, Default)]
pub struct Names {
    /// every entry back to back
    text: Vec<u8>,
    /// where each entry starts in `text`; the last element is the end.
    /// The top bit of an entry's end is set when the entry is not UTF-8
    ends: Vec<u32>,
    /// open addressing over entry numbers plus one (0 is empty); its length
    /// is a power of two, at most half full. Long literals are not entered:
    /// hashing them costs more than the duplicate they might save
    table: Vec<u32>,
    /// entries in `table`
    hashed: u32,
}

/// Literals longer than this are stored without looking for an equal one.
const MAX_HASHED_LEN: usize = 40;

/// The flag on an entry's end that marks it as not UTF-8.
const NOT_TEXT: u32 = 1 << 31;

impl Names {
    /// An empty set sized for a chunk of `src_len` source bytes.
    pub fn with_capacity(src_len: usize) -> Names {
        // about one distinct identifier per 32 source bytes
        let n = (src_len / 32).max(8);
        let mut ends = Vec::with_capacity(n + 1);
        ends.push(0);
        Names {
            text: Vec::with_capacity(n * 6),
            ends,
            table: vec![0; (2 * n).next_power_of_two()],
            hashed: 0,
        }
    }

    /// These buffers emptied for a chunk of `src_len` bytes, keeping what
    /// they have allocated.
    pub(crate) fn reuse(mut self, src_len: usize) -> Names {
        if self.ends.capacity() == 0 {
            return Names::with_capacity(src_len);
        }
        let n = (src_len / 32).max(8);
        let len = (2 * n).next_power_of_two();
        self.text.clear();
        self.ends.clear();
        self.ends.push(0);
        self.table.clear();
        self.table.resize(len, 0);
        self.hashed = 0;
        self
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
    pub fn intern(&mut self, s: &[u8]) -> Sym {
        if self.ends.is_empty() {
            *self = Names::with_capacity(0);
        }
        if s.len() > MAX_HASHED_LEN {
            return self.append(s);
        }
        let mask = self.table.len() - 1;
        let mut i = Self::hash(s) as usize & mask;
        loop {
            match self.table[i] {
                0 => break,
                e => {
                    let sym = Sym(e - 1);
                    if self.bytes(sym) == s {
                        return sym;
                    }
                }
            }
            i = (i + 1) & mask;
        }
        let sym = self.append(s);
        self.table[i] = sym.0 + 1;
        self.hashed += 1;
        if self.hashed as usize * 2 > self.table.len() {
            self.grow();
        }
        sym
    }

    fn append(&mut self, s: &[u8]) -> Sym {
        let sym = Sym(self.ends.len() as u32 - 1);
        self.text.extend_from_slice(s);
        assert!(
            self.text.len() < NOT_TEXT as usize,
            "names of a chunk past 2 GiB"
        );
        let flag = if std::str::from_utf8(s).is_ok() {
            0
        } else {
            NOT_TEXT
        };
        self.ends.push(self.text.len() as u32 | flag);
        sym
    }

    fn grow(&mut self) {
        let len = self.table.len() * 2;
        let mut table = vec![0; len];
        for s in 0..self.ends.len() as u32 - 1 {
            let b = self.bytes(Sym(s));
            if b.len() > MAX_HASHED_LEN {
                continue;
            }
            let mut i = Self::hash(b) as usize & (len - 1);
            while table[i] != 0 {
                i = (i + 1) & (len - 1);
            }
            table[i] = s + 1;
        }
        self.table = table;
    }

    /// The bytes of an entry.
    pub fn bytes(&self, s: Sym) -> &[u8] {
        let i = s.0 as usize;
        &self.text[(self.ends[i] & !NOT_TEXT) as usize..(self.ends[i + 1] & !NOT_TEXT) as usize]
    }

    /// The text of an identifier. Identifiers are ASCII; an entry that is
    /// not UTF-8 (a string literal) reads as the empty string.
    pub fn text(&self, s: Sym) -> &str {
        if self.ends[s.0 as usize + 1] & NOT_TEXT != 0 {
            return "";
        }
        // SAFETY: an entry without the flag was checked to be UTF-8 when it
        // was added
        unsafe { std::str::from_utf8_unchecked(self.bytes(s)) }
    }

    /// The number of entries.
    pub fn len(&self) -> usize {
        self.ends.len().saturating_sub(1)
    }

    /// Whether there are no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_same_number() {
        let mut n = Names::with_capacity(0);
        let words: Vec<String> = (0..200).map(|i| format!("v{}", i % 70)).collect();
        let syms: Vec<Sym> = words.iter().map(|w| n.intern(w.as_bytes())).collect();
        for (w, s) in words.iter().zip(&syms) {
            assert_eq!(n.text(*s), w);
            assert_eq!(n.intern(w.as_bytes()), *s);
        }
        assert_eq!(n.len(), 70);
        assert_ne!(n.intern(b"a"), n.intern(b"b"));
        let e = n.intern(b"");
        assert_eq!(n.text(e), "");
    }

    #[test]
    fn long_literals_are_kept_apart() {
        let mut n = Names::with_capacity(0);
        let long = vec![b'x'; 100];
        let a = n.intern(&long);
        let b = n.intern(&long);
        assert_ne!(a, b);
        assert_eq!(n.bytes(a), &long[..]);
        assert_eq!(n.bytes(b), &long[..]);
        let bin = n.intern(b"\xff\x00");
        assert_eq!(n.bytes(bin), b"\xff\x00");
        assert_eq!(n.text(bin), "");
    }
}
