//! `lua_Debug` as each dialect lays it out, and filling one.
//!
//! `i_ci` holds a level reference: the number of levels from the bottom of
//! the thread's stack up to and including the level, which stays put while
//! calls come and go above it. 0 is 5.1's lost tail call.

use super::super::*;
use luna_core::vm::exec::host_c::HostAr;
use std::collections::HashSet;

/// PUC `LUA_IDSIZE`.
const IDSIZE: usize = 60;

/// 5.1 `lua_Debug`.
#[repr(C)]
struct D51 {
    event: c_int,
    name: *const c_char,
    namewhat: *const c_char,
    what: *const c_char,
    source: *const c_char,
    currentline: c_int,
    nups: c_int,
    linedefined: c_int,
    lastlinedefined: c_int,
    short_src: [c_char; IDSIZE],
    i_ci: c_int,
}

/// 5.2 and 5.3 `lua_Debug`.
#[repr(C)]
struct D52 {
    event: c_int,
    name: *const c_char,
    namewhat: *const c_char,
    what: *const c_char,
    source: *const c_char,
    currentline: c_int,
    linedefined: c_int,
    lastlinedefined: c_int,
    nups: u8,
    nparams: u8,
    isvararg: c_char,
    istailcall: c_char,
    short_src: [c_char; IDSIZE],
    i_ci: *mut c_void,
}

/// 5.4 `lua_Debug`.
#[repr(C)]
struct D54 {
    event: c_int,
    name: *const c_char,
    namewhat: *const c_char,
    what: *const c_char,
    source: *const c_char,
    srclen: usize,
    currentline: c_int,
    linedefined: c_int,
    lastlinedefined: c_int,
    nups: u8,
    nparams: u8,
    isvararg: c_char,
    istailcall: c_char,
    ftransfer: u16,
    ntransfer: u16,
    short_src: [c_char; IDSIZE],
    i_ci: *mut c_void,
}

/// 5.5 `lua_Debug`.
#[repr(C)]
struct D55 {
    event: c_int,
    name: *const c_char,
    namewhat: *const c_char,
    what: *const c_char,
    source: *const c_char,
    srclen: usize,
    currentline: c_int,
    linedefined: c_int,
    lastlinedefined: c_int,
    nups: u8,
    nparams: u8,
    isvararg: c_char,
    extraargs: u8,
    istailcall: c_char,
    ftransfer: c_int,
    ntransfer: c_int,
    short_src: [c_char; IDSIZE],
    i_ci: *mut c_void,
}

/// Room for a `lua_Debug` of any dialect, for the record a hook gets.
#[repr(C)]
pub(in super::super) union ArBuf {
    d51: std::mem::ManuallyDrop<D51>,
    d52: std::mem::ManuallyDrop<D52>,
    d54: std::mem::ManuallyDrop<D54>,
    d55: std::mem::ManuallyDrop<D55>,
}

impl ArBuf {
    pub(in super::super) fn zeroed() -> ArBuf {
        // SAFETY: every field of every layout is an integer, a raw pointer or
        // an array of them, for which all zero bytes is a valid value
        unsafe { std::mem::zeroed() }
    }

    pub(in super::super) fn as_mut_ptr(&mut self) -> *mut c_void {
        (self as *mut ArBuf).cast()
    }
}

/// The strings `lua_getinfo` hands out, kept as long as the thread whose
/// call returned them: PUC's point into strings of the function, which
/// live as long as it does.
#[derive(Default)]
pub(in super::super) struct CStrings(HashSet<Box<[u8]>>);

impl CStrings {
    /// `b` NUL-terminated, at an address that stays put.
    pub(in super::super) fn get(&mut self, b: &[u8]) -> *const c_char {
        let mut z = Vec::with_capacity(b.len() + 1);
        z.extend_from_slice(b);
        z.push(0);
        if let Some(s) = self.0.get(z.as_slice()) {
            return s.as_ptr().cast();
        }
        let s: Box<[u8]> = z.into_boxed_slice();
        let p = s.as_ptr().cast();
        self.0.insert(s);
        p
    }
}

fn short_src(dst: &mut [c_char; IDSIZE], src: &[u8]) {
    let n = src
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(src.len())
        .min(IDSIZE - 1);
    for (d, &s) in dst.iter_mut().zip(&src[..n]) {
        *d = s as c_char;
    }
    dst[n] = 0;
}

/// A `lua_Debug` the host gave, of the state's dialect.
#[derive(Clone, Copy)]
pub(in super::super) struct DebugPtr {
    v: LuaVersion,
    p: *mut c_void,
}

/// The fields every layout shares, filled for options `S`, `l` and `n`.
macro_rules! common {
    ($d:ident, $c:expr, $info:expr, $strs:expr) => {
        match $c {
            b'S' => {
                $d.what = $strs.get($info.what.as_bytes());
                $d.source = $strs.get(&$info.source);
                $d.linedefined = $info.linedefined as c_int;
                $d.lastlinedefined = $info.lastlinedefined as c_int;
                short_src(&mut $d.short_src, &$info.short_src);
                true
            }
            b'l' => {
                $d.currentline = $info.currentline as c_int;
                true
            }
            b'n' => {
                match &$info.name {
                    Some((what, name)) => {
                        $d.namewhat = $strs.get(what.as_bytes());
                        $d.name = $strs.get(name.as_bytes());
                    }
                    None => {
                        $d.namewhat = c"".as_ptr();
                        $d.name = std::ptr::null();
                    }
                }
                true
            }
            b'L' | b'f' => true,
            _ => false,
        }
    };
}

impl DebugPtr {
    /// # Safety
    /// `p` points to a writable `lua_Debug` of dialect `v`.
    pub(in super::super) unsafe fn new(v: LuaVersion, p: *mut c_void) -> DebugPtr {
        DebugPtr { v, p }
    }

    fn d51(&self) -> &mut D51 {
        // SAFETY: `p` is a writable `lua_Debug` of this dialect (`new`), and
        // the borrow ends with the caller's statement
        unsafe { &mut *self.p.cast() }
    }
    fn d52(&self) -> &mut D52 {
        // SAFETY: as `d51`
        unsafe { &mut *self.p.cast() }
    }
    fn d54(&self) -> &mut D54 {
        // SAFETY: as `d51`
        unsafe { &mut *self.p.cast() }
    }
    fn d55(&self) -> &mut D55 {
        // SAFETY: as `d51`
        unsafe { &mut *self.p.cast() }
    }

    /// The level reference in `i_ci`.
    pub(in super::super) fn level_ref(&self) -> usize {
        match self.v {
            LuaVersion::Lua51 => self.d51().i_ci as usize,
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.d52().i_ci as usize,
            LuaVersion::Lua55 => self.d55().i_ci as usize,
            _ => self.d54().i_ci as usize,
        }
    }

    pub(in super::super) fn set_level_ref(&self, r: usize) {
        match self.v {
            LuaVersion::Lua51 => self.d51().i_ci = r as c_int,
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.d52().i_ci = r as *mut c_void,
            LuaVersion::Lua55 => self.d55().i_ci = r as *mut c_void,
            _ => self.d54().i_ci = r as *mut c_void,
        }
    }

    /// `event`, the first field of every layout.
    pub(in super::super) fn event(&self) -> c_int {
        self.d51().event
    }

    pub(in super::super) fn set_event(&self, e: c_int) {
        self.d51().event = e;
    }

    pub(in super::super) fn currentline(&self) -> c_int {
        match self.v {
            LuaVersion::Lua51 => self.d51().currentline,
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.d52().currentline,
            LuaVersion::Lua55 => self.d55().currentline,
            _ => self.d54().currentline,
        }
    }

    pub(in super::super) fn set_currentline(&self, line: c_int) {
        match self.v {
            LuaVersion::Lua51 => self.d51().currentline = line,
            LuaVersion::Lua52 | LuaVersion::Lua53 => self.d52().currentline = line,
            LuaVersion::Lua55 => self.d55().currentline = line,
            _ => self.d54().currentline = line,
        }
    }

    /// Fill the fields option `c` asks for (PUC `auxgetinfo`); `false` for
    /// an option the dialect does not have.
    pub(in super::super) fn fill(&self, c: u8, info: &HostAr, strs: &mut CStrings) -> bool {
        match self.v {
            LuaVersion::Lua51 => {
                let d = self.d51();
                match c {
                    b'u' => {
                        d.nups = info.nups as c_int;
                        true
                    }
                    _ => common!(d, c, info, strs),
                }
            }
            LuaVersion::Lua52 | LuaVersion::Lua53 => {
                let d = self.d52();
                match c {
                    b'u' => {
                        d.nups = info.nups as u8;
                        d.nparams = info.nparams as u8;
                        d.isvararg = c_char::from(info.isvararg);
                        true
                    }
                    b't' => {
                        // PUC stores the `CIST_TAIL` bit itself
                        let bit = if self.v == LuaVersion::Lua52 { 64 } else { 32 };
                        d.istailcall = if info.istailcall { bit } else { 0 };
                        true
                    }
                    _ => common!(d, c, info, strs),
                }
            }
            LuaVersion::Lua55 => {
                let d = self.d55();
                match c {
                    b'S' => {
                        d.srclen = info.source.len();
                        common!(d, c, info, strs)
                    }
                    b'u' => {
                        d.nups = info.nups as u8;
                        d.nparams = info.nparams as u8;
                        d.isvararg = c_char::from(info.isvararg);
                        true
                    }
                    b't' => {
                        d.istailcall = c_char::from(info.istailcall);
                        d.extraargs = info.extraargs as u8;
                        true
                    }
                    b'r' => {
                        d.ftransfer = info.ftransfer as c_int;
                        d.ntransfer = info.ntransfer as c_int;
                        true
                    }
                    _ => common!(d, c, info, strs),
                }
            }
            _ => {
                let d = self.d54();
                match c {
                    b'S' => {
                        d.srclen = info.source.len();
                        common!(d, c, info, strs)
                    }
                    b'u' => {
                        d.nups = info.nups as u8;
                        d.nparams = info.nparams as u8;
                        d.isvararg = c_char::from(info.isvararg);
                        true
                    }
                    b't' => {
                        d.istailcall = if info.istailcall { 32 } else { 0 };
                        true
                    }
                    b'r' => {
                        d.ftransfer = info.ftransfer as u16;
                        d.ntransfer = info.ntransfer as u16;
                        true
                    }
                    _ => common!(d, c, info, strs),
                }
            }
        }
    }

    /// 5.1 `info_tailcall`: every field of a lost tail call, whatever the
    /// options.
    pub(in super::super) fn fill_tail(&self, info: &HostAr, strs: &mut CStrings) {
        for c in [b'S', b'l', b'u'] {
            self.fill(c, info, strs);
        }
        let d = self.d51();
        d.name = c"".as_ptr();
        d.namewhat = c"".as_ptr();
    }
}
