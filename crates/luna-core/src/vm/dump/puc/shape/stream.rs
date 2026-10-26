//! Walking a binary chunk as PUC's undumper reads it, step by step. A step
//! is taken once all the bytes it reads are there; when they run out the
//! walk stops and picks up there when more come, so a chunk handed over
//! piece by piece costs one walk however it is cut.

use super::UEv;

mod layouts;
use layouts::*;
mod steps;
use steps::Reader;

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
    /// the length of a long string the bytes ran out in (5.3 on make it
    /// before they read its bytes)
    pub(crate) cut_long: Option<usize>,
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
            cut_long: None,
            src: None,
            head: None,
            saved: Vec::new(),
            events: events.then(Vec::new),
        }
    }

    /// The events of a walk of `b` that kept them and got to the end.
    pub(crate) fn events<'a>(&self, b: &'a [u8]) -> Option<Vec<UEv<'a>>> {
        self.events.as_ref().filter(|_| self.done)?;
        self.events_so_far(b)
    }

    /// The events of a walk of `b` that kept them, as far as it got.
    pub(crate) fn events_so_far<'a>(&self, b: &'a [u8]) -> Option<Vec<UEv<'a>>> {
        let evs = self.events.as_ref()?;
        let mut out = Vec::with_capacity(evs.len() + 1);
        // 5.1's `luaU_undump` makes the name of a function with no source,
        // once it has read the header
        if self.ver == 0x51 && self.top > 0 {
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
        if self.take_step(r, st, depth)?
            && let Some(f) = self.frames.last_mut()
        {
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
