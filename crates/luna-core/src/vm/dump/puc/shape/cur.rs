//! The cursor the undump walk reads a binary chunk with, and how far a
//! chunk read only in part gets.

use super::*;

pub(super) struct Cur<'a> {
    pub(super) b: &'a [u8],
    pub(super) p: usize,
    pub(super) out: Vec<UEv<'a>>,
    /// where the reading stood at each event
    pub(super) at: Vec<usize>,
    /// the main function's source and its first fields, once read
    pub(super) src: Option<&'a [u8]>,
    pub(super) head: Option<[u64; 4]>,
    /// a long string being read (5.3, 5.4: PUC has it on the stack)
    pub(super) long: Option<usize>,
    /// the bytes the walk needed when they ran out
    pub(super) need: usize,
}

impl<'a> Cur<'a> {
    pub(super) fn ev(&mut self, e: UEv<'a>) {
        self.out.push(e);
        self.at.push(self.p);
    }
    /// The string read since there were `n` events, if one was.
    pub(super) fn read_since(&self, n: usize) -> Option<&'a [u8]> {
        self.out[n..].iter().rev().find_map(|e| match e {
            UEv::Str(s) => Some(*s),
            _ => None,
        })
    }
    /// The main function's line defined, last line, parameters and vararg
    /// flag, read from `f` (4-byte lines) or given.
    pub(super) fn head32(&mut self, depth: u32, f: &[u8]) {
        if depth == 0 {
            let l = |i: usize| u64::from(u32::from_le_bytes([f[i], f[i + 1], f[i + 2], f[i + 3]]));
            self.head = Some([l(0), l(4), u64::from(f[8]), u64::from(f[9])]);
        }
    }
    pub(super) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        self.need = end;
        let s = self.b.get(self.p..end)?;
        self.p += n;
        Some(s)
    }
    /// `take` of a string's bytes: one longer than a short string is on
    /// PUC's stack while they are read.
    pub(super) fn long_take(&mut self, n: usize) -> Option<&'a [u8]> {
        self.long = (n > 40).then_some(n);
        let s = self.take(n)?;
        self.long = None;
        Some(s)
    }
    pub(super) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub(super) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    pub(super) fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// 5.4 `loadUnsigned`: the last byte has the high bit set.
    pub(super) fn var54(&mut self) -> Option<u64> {
        let mut x = 0u64;
        loop {
            let b = self.u8()?;
            x = (x << 7) | u64::from(b & 0x7f);
            if b & 0x80 != 0 {
                return Some(x);
            }
        }
    }
    /// 5.5 `loadVarint`: every byte but the last has the high bit set.
    pub(super) fn var55(&mut self) -> Option<u64> {
        let mut x = 0u64;
        loop {
            let b = self.u8()?;
            x = (x << 7) | u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Some(x);
            }
        }
    }
    pub(super) fn vec(&mut self, a: Arr, n: u64) -> Option<usize> {
        let n = u32::try_from(n).ok()?;
        self.ev(UEv::Vec(a, n));
        Some(n as usize)
    }
    /// 5.5 `loadAlign`: padding to a multiple of `n` from the chunk's start.
    pub(super) fn align(&mut self, n: usize) -> Option<()> {
        let pad = (n - self.p % n) % n;
        self.take(pad).map(|_| ())
    }
}

impl<'a> Cur<'a> {
    pub(super) fn new(b: &'a [u8], p: usize) -> Cur<'a> {
        Cur {
            b,
            p,
            out: Vec::new(),
            at: Vec::new(),
            src: None,
            head: None,
            long: None,
            need: 0,
        }
    }
}

/// How far loading the start `bytes` of a binary chunk gets: its steps,
/// each with the bytes read by then, the main function's source and first
/// fields (line defined, last line, parameters, vararg) when read, and the
/// length of a long string being read when the bytes run out.
pub(crate) struct Progress<'a> {
    pub(crate) events: Vec<(usize, UEv<'a>)>,
    pub(crate) src: Option<&'a [u8]>,
    pub(crate) head: Option<[u64; 4]>,
    pub(crate) long: Option<usize>,
    /// the whole chunk was read
    pub(crate) done: bool,
    /// how many bytes the walk needs to get further, when not done
    pub(crate) need: usize,
}

/// [`undump_events`] of a chunk read only so far.
pub(crate) fn undump_progress(bytes: &[u8], header: usize) -> Progress<'_> {
    let mut c = Cur::new(bytes, header);
    let done = bytes.len() >= header && walk(&mut c).is_some();
    Progress {
        events: c.at.into_iter().zip(c.out).collect(),
        src: c.src,
        head: c.head,
        long: c.long,
        done,
        need: c.need.max(header),
    }
}
