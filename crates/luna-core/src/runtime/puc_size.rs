//! The sizes PUC Lua gives its objects, per dialect and pointer width.
//!
//! `collectgarbage("count")` and the collector's pacing count memory the
//! way PUC's `g->totalbytes` does: each object at the size of PUC's
//! structure for it, whatever luna's own object takes. The sizes are those
//! of PUC built for a target of the same pointer width: 64-bit, or 32-bit
//! as on i386 (where 5.2 packs a value into 8 bytes, its "NaN trick").

use crate::runtime::table::Dialect;

/// The structure sizes of one dialect on one pointer width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PucSizes {
    /// `TValue`: an array slot, a constant, a stack slot
    pub tvalue: usize,
    /// a hash node
    pub node: usize,
    pub table: usize,
    pub upval: usize,
    pub proto: usize,
    pub callinfo: usize,
    /// a thread (`lua_State`, with 5.3+'s extra space in front)
    pub thread: usize,
    pub locvar: usize,
    /// a prototype's upvalue record (5.1: a name pointer)
    pub upvaldesc: usize,
    /// 5.4+ `AbsLineInfo`
    pub abslineinfo: usize,
    pub ptr: usize,
    /// a string's header; a string of `l` bytes takes `str_hdr + l + 1`
    pub str_hdr: usize,
    /// 5.5's long string header (else the same as `str_hdr`)
    pub lngstr_hdr: usize,
    /// a Lua closure takes `lcl_base + lcl_per * n` with `n` upvalues
    pub lcl_base: usize,
    pub lcl_per: usize,
    /// a C closure takes `ccl_base + ccl_per * n`
    pub ccl_base: usize,
    pub ccl_per: usize,
    /// userdata: up to 5.3 a header and the block; 5.4+ the block after
    /// `udata0_off` without user values, else after `udata_uv_off` and the
    /// user values
    pub udata_hdr: usize,
    pub udata0_off: usize,
    pub udata_uv_off: usize,
    pub uvalue: usize,
}

const fn s64(d: Dialect) -> PucSizes {
    // measured on PUC built for x86_64 Linux
    match d {
        Dialect::L51 => PucSizes {
            tvalue: 16,
            node: 40,
            table: 64,
            upval: 40,
            proto: 120,
            callinfo: 40,
            thread: 184,
            locvar: 16,
            upvaldesc: 8,
            abslineinfo: 0,
            ptr: 8,
            str_hdr: 24,
            lngstr_hdr: 24,
            lcl_base: 40,
            lcl_per: 8,
            ccl_base: 40,
            ccl_per: 16,
            udata_hdr: 40,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L52 => PucSizes {
            tvalue: 16,
            node: 40,
            table: 56,
            upval: 40,
            proto: 128,
            callinfo: 80,
            thread: 208,
            locvar: 16,
            upvaldesc: 16,
            abslineinfo: 0,
            ptr: 8,
            str_hdr: 24,
            lngstr_hdr: 24,
            lcl_base: 32,
            lcl_per: 8,
            ccl_base: 32,
            ccl_per: 16,
            udata_hdr: 40,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L53 => PucSizes {
            tvalue: 16,
            node: 32,
            table: 56,
            upval: 32,
            proto: 120,
            callinfo: 72,
            thread: 216,
            locvar: 16,
            upvaldesc: 16,
            abslineinfo: 0,
            ptr: 8,
            str_hdr: 24,
            lngstr_hdr: 24,
            lcl_base: 32,
            lcl_per: 8,
            ccl_base: 32,
            ccl_per: 16,
            udata_hdr: 40,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L54 => PucSizes {
            tvalue: 16,
            node: 24,
            table: 56,
            upval: 40,
            proto: 128,
            callinfo: 64,
            thread: 208,
            locvar: 16,
            upvaldesc: 16,
            abslineinfo: 8,
            ptr: 8,
            str_hdr: 24,
            lngstr_hdr: 24,
            lcl_base: 32,
            lcl_per: 8,
            ccl_base: 32,
            ccl_per: 16,
            udata_hdr: 0,
            udata0_off: 32,
            udata_uv_off: 40,
            uvalue: 16,
        },
        Dialect::L55 => PucSizes {
            tvalue: 16,
            node: 24,
            table: 48,
            upval: 40,
            proto: 128,
            callinfo: 64,
            thread: 216,
            locvar: 16,
            upvaldesc: 16,
            abslineinfo: 8,
            ptr: 8,
            str_hdr: 24,
            lngstr_hdr: 32,
            lcl_base: 32,
            lcl_per: 8,
            ccl_base: 32,
            ccl_per: 16,
            udata_hdr: 0,
            udata0_off: 32,
            udata_uv_off: 48,
            uvalue: 16,
        },
    }
}

const fn s32(d: Dialect) -> PucSizes {
    // measured on PUC built for i386 Linux
    match d {
        Dialect::L51 => PucSizes {
            tvalue: 12,
            node: 28,
            table: 32,
            upval: 24,
            proto: 76,
            callinfo: 24,
            thread: 112,
            locvar: 12,
            upvaldesc: 4,
            abslineinfo: 0,
            ptr: 4,
            str_hdr: 16,
            lngstr_hdr: 16,
            lcl_base: 20,
            lcl_per: 4,
            ccl_base: 20,
            ccl_per: 12,
            udata_hdr: 20,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L52 => PucSizes {
            tvalue: 8,
            node: 20,
            table: 32,
            upval: 20,
            proto: 80,
            callinfo: 40,
            thread: 112,
            locvar: 12,
            upvaldesc: 8,
            abslineinfo: 0,
            ptr: 4,
            str_hdr: 16,
            lngstr_hdr: 16,
            lcl_base: 16,
            lcl_per: 4,
            ccl_base: 16,
            ccl_per: 8,
            udata_hdr: 20,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L53 => PucSizes {
            tvalue: 12,
            node: 28,
            table: 32,
            upval: 20,
            proto: 80,
            callinfo: 36,
            thread: 124,
            locvar: 12,
            upvaldesc: 8,
            abslineinfo: 0,
            ptr: 4,
            str_hdr: 16,
            lngstr_hdr: 16,
            lcl_base: 16,
            lcl_per: 4,
            ccl_base: 16,
            ccl_per: 12,
            udata_hdr: 24,
            udata0_off: 0,
            udata_uv_off: 0,
            uvalue: 0,
        },
        Dialect::L54 => PucSizes {
            tvalue: 12,
            node: 24,
            table: 32,
            upval: 24,
            proto: 84,
            callinfo: 36,
            thread: 120,
            locvar: 12,
            upvaldesc: 8,
            abslineinfo: 8,
            ptr: 4,
            str_hdr: 16,
            lngstr_hdr: 16,
            lcl_base: 16,
            lcl_per: 4,
            ccl_base: 16,
            ccl_per: 12,
            udata_hdr: 0,
            udata0_off: 16,
            udata_uv_off: 20,
            uvalue: 12,
        },
        Dialect::L55 => PucSizes {
            tvalue: 12,
            node: 24,
            table: 28,
            upval: 24,
            proto: 84,
            callinfo: 36,
            thread: 128,
            locvar: 12,
            upvaldesc: 8,
            abslineinfo: 8,
            ptr: 4,
            str_hdr: 16,
            lngstr_hdr: 20,
            lcl_base: 16,
            lcl_per: 4,
            ccl_base: 16,
            ccl_per: 12,
            udata_hdr: 0,
            udata0_off: 16,
            udata_uv_off: 20,
            uvalue: 12,
        },
    }
}

impl PucSizes {
    /// The sizes for `d` on the pointer width luna is built for.
    pub(crate) const fn of(d: Dialect) -> PucSizes {
        if cfg!(target_pointer_width = "64") {
            s64(d)
        } else {
            s32(d)
        }
    }

    /// A string of `len` bytes; `long` from 5.5 on uses the long header.
    pub(crate) fn string(&self, len: usize, long: bool) -> usize {
        let hdr = if long { self.lngstr_hdr } else { self.str_hdr };
        hdr + len + 1
    }

    pub(crate) fn lua_closure(&self, nupvals: usize) -> usize {
        self.lcl_base + self.lcl_per * nupvals
    }

    pub(crate) fn c_closure(&self, nupvals: usize) -> usize {
        self.ccl_base + self.ccl_per * nupvals
    }

    /// A userdata of `nbytes` with `nuv` user values (up to 5.3 always one
    /// user value, kept in the header).
    pub(crate) fn userdata(&self, nuv: usize, nbytes: usize) -> usize {
        if self.udata_hdr != 0 {
            self.udata_hdr + nbytes
        } else if nuv == 0 {
            self.udata0_off + nbytes
        } else {
            self.udata_uv_off + self.uvalue * nuv + nbytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // values PUC's own macros give on x86_64 Linux
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn formulas_match_puc_macros_on_64_bit() {
        let s = PucSizes::of(Dialect::L51);
        assert_eq!(
            [
                s.lua_closure(0),
                s.lua_closure(3),
                s.c_closure(1),
                s.c_closure(3)
            ],
            [40, 64, 56, 88]
        );
        let s = PucSizes::of(Dialect::L54);
        assert_eq!([s.lua_closure(2), s.c_closure(2)], [48, 64]);
        assert_eq!([s.userdata(0, 10), s.userdata(2, 10)], [42, 82]);
        assert_eq!(s.string(3, false), 28);
        let s = PucSizes::of(Dialect::L55);
        assert_eq!([s.string(41, true), s.string(40, false)], [74, 65]);
    }
}
