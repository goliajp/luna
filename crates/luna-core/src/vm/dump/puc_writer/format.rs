//! Chunk layout of each PUC version, as its `ldump.c` writes it: 5.1–5.3
//! with fixed-size integers, 5.4 and 5.5 with varints (of opposite
//! conventions), 5.5 with each string saved once and 4-byte alignment of
//! the code and absolute line info.

use super::{Dialect, Out};
use crate::vm::dump::puc::{puc_51, puc_52, puc_53, puc_54, puc_55};
use std::collections::HashMap;

pub(super) fn write(d: Dialect, root: &Out, strip: bool) -> Vec<u8> {
    let mut w = W {
        out: Vec::new(),
        strip,
        saved: HashMap::new(),
    };
    match d {
        Dialect::V51 => {
            w.out.extend_from_slice(puc_51::HEADER);
            w.f51(root, None);
        }
        Dialect::V52 => {
            w.out.extend_from_slice(puc_52::HEADER);
            w.f52(root);
        }
        Dialect::V53 => {
            w.out.extend_from_slice(puc_53::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f53(root, None);
        }
        Dialect::V54 => {
            w.out.extend_from_slice(puc_54::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f54(root, None);
        }
        Dialect::V55 => {
            w.out.extend_from_slice(puc_55::HEADER);
            w.out.push(root.upvals.len() as u8);
            w.f55(root);
        }
    }
    w.out
}

struct W {
    out: Vec<u8>,
    strip: bool,
    /// 5.5: strings written so far, by content, with their 1-based index
    saved: HashMap<Vec<u8>, u64>,
}

/// The source a nested function writes: none when it is its parent's (the
/// loader inherits it) or when stripping.
fn own_source<'a>(p: &'a Out, parent: Option<&[u8]>, strip: bool) -> Option<&'a [u8]> {
    (!strip && parent != Some(&p.source[..])).then_some(&p.source[..])
}

impl W {
    fn byte(&mut self, b: u8) {
        self.out.push(b);
    }

    fn int(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn code(&mut self, code: &[u32]) {
        for &i in code {
            self.int(i);
        }
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
