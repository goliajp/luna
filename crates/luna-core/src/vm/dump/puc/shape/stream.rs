//! Walking a binary chunk as PUC's undumper reads it, step by step. A step
//! is taken once all the bytes it reads are there; when they run out the
//! walk stops and picks up there when more come, so a chunk handed over
//! piece by piece costs one walk however it is cut.

use super::UEv;

#[path = "layouts.rs"]
mod layouts;
use layouts::*;

/// A layout being walked: a function's, or a list's elements with `left`
/// more after this one.
struct Frame {
    prog: &'static [St],
    pc: usize,
    depth: u32,
    /// the last count read, and the function's upvalues
    n: u64,
    ups: u64,
    left: Option<u64>,
}

/// An event of the walk, its strings as where they are in the bytes.
#[derive(Clone, Copy)]
enum Ev {
    U(UEv<'static>),
    Str(usize, usize),
}

/// How far PUC's undumper has got with the bytes handed over so far.
pub(crate) struct Undumping {
    ver: u8,
    p: usize,
    frames: Vec<Frame>,
    /// 0 the header, 1 the closure, 2 the main function's layout
    top: u8,
    /// the bytes the step it stopped at reads
    pub(crate) need: usize,
    pub(crate) done: bool,
    /// the bytes are not a PUC binary chunk the walk can read
    pub(crate) bad: bool,
    /// the prototypes being read, each by the number of those made before
    pub(crate) open: Vec<u32>,
    made: u32,
    /// the main closure's upvalues, once made
    pub(crate) closure: Option<u8>,
    /// a long string being read (5.3, 5.4 hold it on the stack), and
    /// where its bytes start
    pub(crate) long: Option<usize>,
    pub(crate) long_at: usize,
    /// the main function's source, where it is in the bytes, and its first
    /// fields (line defined, last line, parameters, vararg), once read
    pub(crate) src: Option<(usize, usize)>,
    pub(crate) head: Option<[u64; 4]>,
    /// 5.5: the strings saved, in order
    pub(crate) saved: Vec<(usize, usize)>,
    /// every event, when they are kept
    events: Option<Vec<Ev>>,
}

impl Undumping {
    /// A walk that keeps what the stack needs; `events`: every event too.
    pub(crate) fn new(events: bool) -> Undumping {
        Undumping {
            ver: 0,
            p: 0,
            frames: Vec::new(),
            top: 0,
            need: 0,
            done: false,
            bad: false,
            open: Vec::new(),
            made: 0,
            closure: None,
            long: None,
            long_at: 0,
            src: None,
            head: None,
            saved: Vec::new(),
            events: events.then(Vec::new),
        }
    }

    /// The events of a walk of `b` that kept them and got to the end.
    pub(crate) fn events<'a>(&self, b: &'a [u8]) -> Option<Vec<UEv<'a>>> {
        let evs = self.events.as_ref().filter(|_| self.done)?;
        let mut out = Vec::with_capacity(evs.len() + 1);
        // 5.1's `luaU_undump` makes the name of a function with no source
        if self.ver == 0x51 {
            out.push(UEv::Str(b"=?"));
        }
        for &e in evs {
            out.push(match e {
                Ev::U(UEv::Str(_)) => unreachable!("strings are kept by place"),
                Ev::U(UEv::Closure(n)) => UEv::Closure(n),
                Ev::U(UEv::Reclose(n)) => UEv::Reclose(n),
                Ev::U(UEv::Proto) => UEv::Proto,
                Ev::U(UEv::End) => UEv::End,
                Ev::U(UEv::Vec(a, n)) => UEv::Vec(a, n),
                Ev::U(UEv::Buff(n)) => UEv::Buff(n),
                Ev::Str(at, len) => UEv::Str(b.get(at..at + len)?),
            });
        }
        Some(out)
    }

    fn ev(&mut self, e: UEv<'static>) {
        if let Some(evs) = &mut self.events {
            evs.push(Ev::U(e));
        }
    }

    /// Walk on with `b`, all the bytes handed over so far, as far as they
    /// go.
    pub(crate) fn feed(&mut self, b: &[u8]) {
        if self.done || self.bad || b.len() < self.need {
            return;
        }
        if self.ver == 0 {
            match b.get(4) {
                None => {
                    self.need = 5;
                    return;
                }
                Some(&v) if (0x51..=0x55).contains(&v) => self.ver = v,
                Some(_) => {
                    self.bad = true;
                    return;
                }
            }
        }
        while !self.done && !self.bad {
            let mut r = Reader {
                b,
                p: self.p,
                need: 0,
            };
            let mark = self.events.as_ref().map_or(0, Vec::len);
            if self.step(&mut r).is_some() {
                self.p = r.p;
            } else {
                // a step that read past the bytes made nothing: taken again
                if let Some(evs) = &mut self.events {
                    evs.truncate(mark);
                }
                self.need = r.need;
                if r.need <= b.len() {
                    self.bad = true;
                }
                return;
            }
        }
    }

    /// One step, all of it, or nothing when its bytes are not all there.
    fn step(&mut self, r: &mut Reader) -> Option<()> {
        match self.top {
            0 => {
                r.take(header_len(self.ver))?;
                self.top = 1;
                return Some(());
            }
            1 => {
                let n = match self.ver {
                    0x51 => 0,
                    0x52 => 1,
                    _ => r.u8()?,
                };
                if self.ver != 0x51 {
                    self.closure = Some(n);
                    self.ev(UEv::Closure(n));
                    self.open.push(self.made);
                    self.made += 1;
                    self.ev(UEv::Proto);
                }
                self.frames.push(Frame {
                    prog: prog(self.ver),
                    pc: 0,
                    depth: 0,
                    n: 0,
                    ups: 0,
                    left: None,
                });
                self.top = 2;
                return Some(());
            }
            _ => {}
        }
        let Some(f) = self.frames.last_mut() else {
            self.done = true;
            return Some(());
        };
        let Some(&st) = f.prog.get(f.pc) else {
            let f = self.frames.pop().expect("a frame");
            match f.left {
                Some(left) if left > 0 => {
                    self.frames.push(Frame {
                        pc: 0,
                        left: Some(left - 1),
                        ..f
                    });
                }
                Some(_) => {}
                None if f.depth == 0 => {
                    // 5.2 makes the closure again for the function's upvalues
                    if self.ver == 0x52 && f.ups != 1 {
                        self.closure = Some(f.ups as u8);
                        self.ev(UEv::Reclose(f.ups as u8));
                    }
                    self.done = true;
                }
                None => {}
            }
            return Some(());
        };
        let depth = f.depth;
        match st {
            Proto => {
                self.open.push(self.made);
                self.made += 1;
                self.ev(UEv::Proto);
            }
            End => {
                self.open.pop();
                self.ev(UEv::End);
            }
            Str(sv, src) => {
                let s = self.string(r, sv)?;
                if src && depth == 0 {
                    self.src = Some(s.unwrap_or((0, 0)));
                }
            }
            Take(n) => {
                r.take(n)?;
            }
            Head(four, k) => {
                let (line, last, h) = match four {
                    Some(()) => {
                        let h = r.take(11)?;
                        let l = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
                        (u64::from(l(0)), u64::from(l(4)), &h[8..])
                    }
                    None => (r.num(k)?, r.num(k)?, r.take(3)?),
                };
                if depth == 0 {
                    self.head = Some([line, last, u64::from(h[0]), u64::from(h[1])]);
                }
            }
            Count(k, arr) => {
                let n = r.num(k)?;
                if let Some(a) = arr {
                    self.ev(UEv::Vec(a, u32::try_from(n).ok()?));
                }
                self.frames.last_mut().expect("a frame").n = n;
            }
            VecOf(a) => {
                let n = self.frames.last().expect("a frame").n;
                self.ev(UEv::Vec(a, u32::try_from(n).ok()?));
            }
            TakeN(m) => {
                let n = self.frames.last().expect("a frame").n;
                r.take(usize::try_from(n).ok()?.checked_mul(m)?)?;
            }
            Var(k) => {
                r.num(k)?;
            }
            Align4 => {
                r.take((4 - r.p % 4) % 4)?;
            }
            Const(sv) => match (sv, r.u8()?) {
                (_, 0) | (Sv::S54 | Sv::S55, 1 | 17) => {}
                (Sv::S51 | Sv::S53, 1) => {
                    r.u8()?;
                }
                (Sv::S55, 3) => {
                    r.num(V55)?;
                }
                (_, 3 | 19) => {
                    r.take(8)?;
                }
                (_, 4 | 20) => {
                    self.string(r, sv)?;
                }
                _ => {
                    self.bad = true;
                    return Some(());
                }
            },
            Ups => {
                let f = self.frames.last_mut().expect("a frame");
                f.ups = f.n;
            }
            AbsLines => {
                let n = r.num(V55)?;
                if n > 0 {
                    r.take((4 - r.p % 4) % 4)?;
                    self.ev(UEv::Vec(A::AbsLines, u32::try_from(n).ok()?));
                    r.take(usize::try_from(n).ok()?.checked_mul(8)?)?;
                }
            }
            Names => {
                let ups = if r.num(V55)? != 0 {
                    self.frames.last().expect("a frame").ups
                } else {
                    0
                };
                return Some(self.enter(&[Str(Sv::S55, false)], ups, depth));
            }
            Each(body) => {
                let n = self.frames.last().expect("a frame").n;
                return Some(self.enter(body, n, depth));
            }
            Funcs => {
                let n = self.frames.last().expect("a frame").n;
                return Some(self.enter(&[Fn], n, depth));
            }
            Fn => {
                self.frames.last_mut().expect("a frame").pc += 1;
                if depth >= DEEP {
                    self.bad = true;
                    return Some(());
                }
                if self.ver != 0x51 {
                    self.open.push(self.made);
                    self.made += 1;
                    self.ev(UEv::Proto);
                }
                self.frames.push(Frame {
                    prog: prog(self.ver),
                    pc: 0,
                    depth: depth + 1,
                    n: 0,
                    ups: 0,
                    left: None,
                });
                return Some(());
            }
        }
        if let Some(f) = self.frames.last_mut() {
            f.pc += 1;
        }
        Some(())
    }

    /// Past the step that starts a list: its `n` elements of `body` next.
    fn enter(&mut self, body: &'static [St], n: u64, depth: u32) {
        self.frames.last_mut().expect("a frame").pc += 1;
        if n > 0 {
            self.frames.push(Frame {
                prog: body,
                pc: 0,
                depth,
                n: 0,
                ups: 0,
                left: Some(n - 1),
            });
        }
    }

    /// A string: where it is in the bytes (`None`: none, a size of 0).
    fn string(&mut self, r: &mut Reader, sv: Sv) -> Option<Option<(usize, usize)>> {
        let n = match sv {
            Sv::S51 => r.u64()?,
            Sv::S53 => match r.u8()? {
                0xFF => r.u64()?,
                n => u64::from(n),
            },
            Sv::S54 => r.num(V54)?,
            Sv::S55 => r.num(V55)?,
        };
        if n == 0 {
            if matches!(sv, Sv::S55) {
                // a saved string again
                r.num(V55)?;
            }
            return Some(None);
        }
        let len = usize::try_from(n).ok()? - 1;
        // 5.1, 5.2 and 5.5 store the terminating zero
        let bytes = if matches!(sv, Sv::S51 | Sv::S55) {
            len + 1
        } else {
            len
        };
        let at = r.p;
        if r.take(bytes).is_none() {
            // a long string is on 5.3's and 5.4's stack while it is read
            self.long = (matches!(sv, Sv::S53 | Sv::S54) && len > 40).then_some(len);
            self.long_at = at;
            return None;
        }
        self.long = None;
        if matches!(sv, Sv::S51) {
            self.ev(UEv::Buff(u32::try_from(n).ok()?));
        }
        if let Some(evs) = &mut self.events {
            evs.push(Ev::Str(at, len));
        }
        if matches!(sv, Sv::S55) {
            self.saved.push((at, len));
        }
        Some(Some((at, len)))
    }
}

/// The function layout of format `v`.
fn prog(v: u8) -> &'static [St] {
    [F51, F52, F53, F54, F55][usize::from(v - 0x51)]
}

/// The header's length in format `v`.
fn header_len(v: u8) -> usize {
    use crate::vm::dump::puc;
    match v {
        0x51 => puc::puc_51::HEADER.len(),
        0x52 => puc::puc_52::HEADER.len(),
        0x53 => puc::puc_53::HEADER.len(),
        0x54 => puc::puc_54::HEADER.len(),
        _ => puc::puc_55::HEADER.len(),
    }
}

/// A cursor over the bytes that notes what a read that ran out needed.
struct Reader<'a> {
    b: &'a [u8],
    p: usize,
    need: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        self.need = end;
        let s = self.b.get(self.p..end)?;
        self.p = end;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn num(&mut self, k: Num) -> Option<u64> {
        if let Num::U32 = k {
            return Some(u64::from(u32::from_le_bytes(
                self.take(4)?.try_into().ok()?,
            )));
        }
        let mut x = 0u64;
        loop {
            let b = self.u8()?;
            x = (x << 7) | u64::from(b & 0x7f);
            let last = match k {
                Num::V54 => b & 0x80 != 0,
                _ => b & 0x80 == 0,
            };
            if last {
                return Some(x);
            }
        }
    }
}
