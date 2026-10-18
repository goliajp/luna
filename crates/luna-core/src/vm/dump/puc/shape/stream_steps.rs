//! The steps of the walk that read a function's fields.

use super::*;

impl Undumping {
    /// Step `st` of a layout at `depth`, all of it, or nothing when its
    /// bytes are not all there; whether the layout goes on to its next
    /// step (not when it entered a list or a function, or the bytes are
    /// bad).
    pub(super) fn take_step(&mut self, r: &mut Reader, st: St, depth: u32) -> Option<bool> {
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
                    return Some(false);
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
                self.enter(&[Str(Sv::S55, false)], ups, depth);
                return Some(false);
            }
            Each(body) => {
                let n = self.frames.last().expect("a frame").n;
                self.enter(body, n, depth);
                return Some(false);
            }
            Funcs => {
                let n = self.frames.last().expect("a frame").n;
                self.enter(&[Fn], n, depth);
                return Some(false);
            }
            Fn => {
                self.frames.last_mut().expect("a frame").pc += 1;
                if depth >= DEEP {
                    self.bad = true;
                    return Some(false);
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
                return Some(false);
            }
        }
        Some(true)
    }

    /// A string: where it is in the bytes (`None`: none, a size of 0).
    pub(super) fn string(&mut self, r: &mut Reader, sv: Sv) -> Option<Option<(usize, usize)>> {
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

/// A cursor over the bytes that notes what a read that ran out needed.
pub(super) struct Reader<'a> {
    pub(super) b: &'a [u8],
    pub(super) p: usize,
    pub(super) need: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        self.need = end;
        let s = self.b.get(self.p..end)?;
        self.p = end;
        Some(s)
    }
    pub(super) fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub(super) fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    pub(super) fn num(&mut self, k: Num) -> Option<u64> {
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
