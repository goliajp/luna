//! What PUC's undumper allocates loading a binary chunk, in its order:
//! read again from the chunk's bytes, which the translator has accepted.

/// One allocation step of `luaU_undump` (with what `f_parser` does around
/// it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum UEv<'a> {
    /// a Lua closure of this many upvalues (`luaF_newLclosure`)
    Closure(u8),
    /// 5.2: the closure made with one upvalue is replaced by one of this
    /// many
    Reclose(u8),
    /// a prototype (`luaF_newproto`), its function read up to [`UEv::End`]
    Proto,
    /// the function read since the matching [`UEv::Proto`] ends
    End,
    /// an array of the innermost function (`luaM_newvector`)
    Vec(Arr, u32),
    /// a string (5.5: saved for reuse, at the next index of the list)
    Str(&'a [u8]),
    /// 5.1 and 5.2: the load buffer grown to hold a string (`luaZ_openspace`)
    Buff(u32),
}

/// The arrays of a prototype, in [`crate::runtime::Proto::puc_arrays`]
/// order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Arr {
    Code,
    Protos,
    Consts,
    Lines,
    AbsLines,
    LocVars,
    Upvals,
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
    out: Vec<UEv<'a>>,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p.checked_add(n)?)?;
        self.p += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// 5.4 `loadUnsigned`: the last byte has the high bit set.
    fn var54(&mut self) -> Option<u64> {
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
    fn var55(&mut self) -> Option<u64> {
        let mut x = 0u64;
        loop {
            let b = self.u8()?;
            x = (x << 7) | u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Some(x);
            }
        }
    }
    fn vec(&mut self, a: Arr, n: u64) -> Option<usize> {
        let n = u32::try_from(n).ok()?;
        self.out.push(UEv::Vec(a, n));
        Some(n as usize)
    }
    /// 5.5 `loadAlign`: padding to a multiple of `n` from the chunk's start.
    fn align(&mut self, n: usize) -> Option<()> {
        let pad = (n - self.p % n) % n;
        self.take(pad).map(|_| ())
    }
}

/// The steps of loading `bytes`, a binary chunk of dialect `version` (the
/// version byte) whose header is `header` bytes long; `None` if it does
/// not read as one.
pub(crate) fn undump_events(bytes: &[u8], header: usize) -> Option<Vec<UEv<'_>>> {
    let mut c = Cur {
        b: bytes,
        p: header,
        out: Vec::new(),
    };
    match bytes.get(4)? {
        0x51 => {
            // `luaU_undump` makes the name of a function with no source
            c.out.push(UEv::Str(b"=?"));
            f51(&mut c, 0)?;
        }
        0x52 => {
            c.out.push(UEv::Closure(1));
            c.out.push(UEv::Proto);
            let ups = f52(&mut c, 0)?;
            if ups != 1 {
                c.out.push(UEv::Reclose(ups));
            }
        }
        0x53 => {
            let n = c.u8()?;
            c.out.push(UEv::Closure(n));
            c.out.push(UEv::Proto);
            f53(&mut c, 0)?;
        }
        0x54 => {
            let n = c.u8()?;
            c.out.push(UEv::Closure(n));
            c.out.push(UEv::Proto);
            f54(&mut c, 0)?;
        }
        0x55 => {
            let n = c.u8()?;
            c.out.push(UEv::Closure(n));
            c.out.push(UEv::Proto);
            f55(&mut c, 0)?;
        }
        _ => return None,
    }
    Some(c.out)
}

const DEEP: u32 = 250;

/// 5.1 `LoadString`: a size and that many bytes with the NUL.
fn s51(c: &mut Cur) -> Option<()> {
    let n = c.u64()? as usize;
    if n > 0 {
        c.out.push(UEv::Buff(u32::try_from(n).ok()?));
        let s = c.take(n)?;
        c.out.push(UEv::Str(&s[..n - 1]));
    }
    Some(())
}

/// 5.1 `LoadFunction`.
fn f51(c: &mut Cur, depth: u32) -> Option<()> {
    if depth > DEEP {
        return None;
    }
    c.out.push(UEv::Proto);
    s51(c)?;
    c.take(4 + 4 + 4)?;
    let n = c.u32()?;
    let n = c.vec(Arr::Code, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Consts, n.into())? {
        match c.u8()? {
            0 => {}
            1 => {
                c.u8()?;
            }
            3 => {
                c.u64()?;
            }
            4 => s51(c)?,
            _ => return None,
        }
    }
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Protos, n.into())? {
        f51(c, depth + 1)?;
    }
    let n = c.u32()?;
    let n = c.vec(Arr::Lines, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::LocVars, n.into())? {
        s51(c)?;
        c.take(8)?;
    }
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Upvals, n.into())? {
        s51(c)?;
    }
    c.out.push(UEv::End);
    Some(())
}

/// 5.2 `LoadFunction` (the prototype made by the caller); its upvalues.
fn f52(c: &mut Cur, depth: u32) -> Option<u8> {
    if depth > DEEP {
        return None;
    }
    c.take(4 + 4 + 3)?;
    let n = c.u32()?;
    let n = c.vec(Arr::Code, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Consts, n.into())? {
        match c.u8()? {
            0 => {}
            1 => {
                c.u8()?;
            }
            3 => {
                c.u64()?;
            }
            4 => s51(c)?,
            _ => return None,
        }
    }
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Protos, n.into())? {
        c.out.push(UEv::Proto);
        f52(c, depth + 1)?;
    }
    let n = c.u32()?;
    let ups = c.vec(Arr::Upvals, n.into())?;
    c.take(2 * ups)?;
    // the source, then the debug information
    s51(c)?;
    let n = c.u32()?;
    let n = c.vec(Arr::Lines, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::LocVars, n.into())? {
        s51(c)?;
        c.take(8)?;
    }
    for _ in 0..c.u32()? {
        s51(c)?;
    }
    c.out.push(UEv::End);
    u8::try_from(ups).ok()
}

/// 5.3 `LoadString`: a byte size (0xFF: a `size_t` follows), the bytes
/// without the NUL.
fn s53(c: &mut Cur) -> Option<()> {
    let mut n = u64::from(c.u8()?);
    if n == 0xFF {
        n = c.u64()?;
    }
    if n > 0 {
        let s = c.take(n as usize - 1)?;
        c.out.push(UEv::Str(s));
    }
    Some(())
}

/// 5.3 `LoadFunction` (the prototype made by the caller).
fn f53(c: &mut Cur, depth: u32) -> Option<()> {
    if depth > DEEP {
        return None;
    }
    s53(c)?;
    c.take(4 + 4 + 3)?;
    let n = c.u32()?;
    let n = c.vec(Arr::Code, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Consts, n.into())? {
        match c.u8()? {
            0 => {}
            1 => {
                c.u8()?;
            }
            3 | 19 => {
                c.u64()?;
            }
            4 | 20 => s53(c)?,
            _ => return None,
        }
    }
    let n = c.u32()?;
    let n = c.vec(Arr::Upvals, n.into())?;
    c.take(2 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::Protos, n.into())? {
        c.out.push(UEv::Proto);
        f53(c, depth + 1)?;
    }
    let n = c.u32()?;
    let n = c.vec(Arr::Lines, n.into())?;
    c.take(4 * n)?;
    let n = c.u32()?;
    for _ in 0..c.vec(Arr::LocVars, n.into())? {
        s53(c)?;
        c.take(8)?;
    }
    for _ in 0..c.u32()? {
        s53(c)?;
    }
    c.out.push(UEv::End);
    Some(())
}

/// 5.4 `loadStringN`.
fn s54(c: &mut Cur) -> Option<()> {
    let n = c.var54()?;
    if n > 0 {
        let s = c.take(n as usize - 1)?;
        c.out.push(UEv::Str(s));
    }
    Some(())
}

/// 5.4 `loadFunction` (the prototype made by the caller).
fn f54(c: &mut Cur, depth: u32) -> Option<()> {
    if depth > DEEP {
        return None;
    }
    s54(c)?;
    c.var54()?;
    c.var54()?;
    c.take(3)?;
    let n = c.var54()?;
    let n = c.vec(Arr::Code, n)?;
    c.take(4 * n)?;
    let n = c.var54()?;
    for _ in 0..c.vec(Arr::Consts, n)? {
        match c.u8()? {
            0 | 1 | 17 => {}
            3 | 19 => {
                c.u64()?;
            }
            4 | 20 => s54(c)?,
            _ => return None,
        }
    }
    let n = c.var54()?;
    let n = c.vec(Arr::Upvals, n)?;
    c.take(3 * n)?;
    let n = c.var54()?;
    for _ in 0..c.vec(Arr::Protos, n)? {
        c.out.push(UEv::Proto);
        f54(c, depth + 1)?;
    }
    let n = c.var54()?;
    let n = c.vec(Arr::Lines, n)?;
    c.take(n)?;
    let n = c.var54()?;
    for _ in 0..c.vec(Arr::AbsLines, n)? {
        c.var54()?;
        c.var54()?;
    }
    let n = c.var54()?;
    for _ in 0..c.vec(Arr::LocVars, n)? {
        s54(c)?;
        c.var54()?;
        c.var54()?;
    }
    for _ in 0..c.var54()? {
        s54(c)?;
    }
    c.out.push(UEv::End);
    Some(())
}

/// 5.5 `loadString`: a reference to a saved string (no new one) or a new
/// string, saved.
fn s55(c: &mut Cur) -> Option<()> {
    let n = c.var55()?;
    if n == 0 {
        c.var55()?;
    } else {
        let s = c.take(n as usize)?;
        c.out.push(UEv::Str(&s[..n as usize - 1]));
    }
    Some(())
}

/// 5.5 `loadFunction` (the prototype made by the caller).
fn f55(c: &mut Cur, depth: u32) -> Option<()> {
    if depth > DEEP {
        return None;
    }
    c.var55()?;
    c.var55()?;
    c.take(3)?;
    let n = c.var55()?;
    c.align(4)?;
    let n = c.vec(Arr::Code, n)?;
    c.take(4 * n)?;
    let n = c.var55()?;
    for _ in 0..c.vec(Arr::Consts, n)? {
        match c.u8()? {
            0 | 1 | 17 => {}
            19 => {
                c.u64()?;
            }
            3 => {
                c.var55()?;
            }
            4 | 20 => s55(c)?,
            _ => return None,
        }
    }
    let n = c.var55()?;
    let ups = c.vec(Arr::Upvals, n)?;
    c.take(3 * ups)?;
    let n = c.var55()?;
    for _ in 0..c.vec(Arr::Protos, n)? {
        c.out.push(UEv::Proto);
        f55(c, depth + 1)?;
    }
    s55(c)?;
    let n = c.var55()?;
    let n = c.vec(Arr::Lines, n)?;
    c.take(n)?;
    let n = c.var55()?;
    if n > 0 {
        c.align(4)?;
        let n = c.vec(Arr::AbsLines, n)?;
        c.take(8 * n)?;
    }
    let n = c.var55()?;
    for _ in 0..c.vec(Arr::LocVars, n)? {
        s55(c)?;
        c.var55()?;
        c.var55()?;
    }
    if c.var55()? != 0 {
        for _ in 0..ups {
            s55(c)?;
        }
    }
    c.out.push(UEv::End);
    Some(())
}
