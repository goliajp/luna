//! Chunk layout of each PUC version, as its `ldump.c` writes it: 5.1–5.3
//! with fixed-size integers, 5.4 and 5.5 with varints (of opposite
//! conventions), 5.5 with each string saved once and 4-byte alignment of
//! the code and absolute line info.

use super::{Dialect, Out};
use crate::vm::dump::puc::{puc_51, puc_52, puc_53, puc_54, puc_55};
use std::collections::HashMap;

/// The chunk, and the size of each block PUC's `ldump.c` hands its writer
/// in turn (one `DumpBlock` / `dumpBlock` call each), so `lua_dump` can
/// call the host's writer the same way.
pub(super) fn write(d: Dialect, root: &Out, strip: bool) -> (Vec<u8>, Vec<usize>) {
    let mut w = W {
        out: Vec::new(),
        strip,
        saved: HashMap::new(),
        pieces: Vec::new(),
        mark: 0,
        // 5.3 and 5.4 skip empty blocks; the others call the writer anyway
        zero: !matches!(d, Dialect::V53 | Dialect::V54),
    };
    match d {
        Dialect::V51 => {
            w.header(puc_51::HEADER, &[12]);
            w.f51(root, None);
        }
        Dialect::V52 => {
            w.header(puc_52::HEADER, &[18]);
            w.f52(root);
        }
        Dialect::V53 => {
            w.header(puc_53::HEADER, &[4, 1, 1, 6, 1, 1, 1, 1, 1, 8, 8]);
            w.byte(root.upvals.len() as u8);
            w.f53(root, None);
        }
        Dialect::V54 => {
            w.header(puc_54::HEADER, &[4, 1, 1, 6, 1, 1, 1, 8, 8]);
            w.byte(root.upvals.len() as u8);
            w.f54(root, None);
        }
        Dialect::V55 => {
            w.header(puc_55::HEADER, &[4, 1, 1, 6, 1, 4, 1, 4, 1, 8, 1, 8]);
            w.byte(root.upvals.len() as u8);
            w.f55(root);
        }
    }
    (w.out, w.pieces)
}

struct W {
    out: Vec<u8>,
    strip: bool,
    /// 5.5: strings written so far, by content, with their 1-based index
    saved: HashMap<Vec<u8>, u64>,
    /// the size of each block written so far
    pieces: Vec<usize>,
    /// where the block being written starts
    mark: usize,
    /// whether an empty block counts as a block
    zero: bool,
}

/// The source a nested function writes: none when it is its parent's (the
/// loader inherits it) or when stripping.
fn own_source<'a>(p: &'a Out, parent: Option<&[u8]>, strip: bool) -> Option<&'a [u8]> {
    (!strip && parent != Some(&p.source[..])).then_some(&p.source[..])
}

impl W {
    /// End the block written since the last one.
    fn cut(&mut self) {
        let n = self.out.len() - self.mark;
        if n > 0 || self.zero {
            self.pieces.push(n);
        }
        self.mark = self.out.len();
    }

    /// The header, in blocks of the sizes `sizes`.
    fn header(&mut self, h: &[u8], sizes: &[usize]) {
        debug_assert_eq!(sizes.iter().sum::<usize>(), h.len());
        let mut at = 0;
        for &n in sizes {
            self.bytes(&h[at..at + n]);
            at += n;
        }
    }

    /// One block.
    fn bytes(&mut self, b: &[u8]) {
        self.out.extend_from_slice(b);
        self.cut();
    }

    fn byte(&mut self, b: u8) {
        self.bytes(&[b]);
    }

    fn int(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    /// A vector of 32-bit values as one block.
    fn ints(&mut self, v: &[u32]) {
        for &i in v {
            self.out.extend_from_slice(&i.to_le_bytes());
        }
        self.cut();
    }

    fn code(&mut self, code: &[u32]) {
        self.ints(code);
    }

    /// Debug tables are empty when stripping.
    fn debug_len<T>(&self, v: &[T]) -> usize {
        if self.strip { 0 } else { v.len() }
    }
}

mod classic;
mod modern;

#[cfg(test)]
mod tests;
