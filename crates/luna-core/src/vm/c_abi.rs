//! The C types of the target, as PUC is built for it, and C's conversions
//! to them as x86 makes them with SSE2: what PUC's library functions see.
//! On a 64-bit target each conversion is the plain cast luna has always
//! made there; a 32-bit target (i686, with PUC built `-msse2
//! -mfpmath=sse`) has a 32-bit `long`, `size_t` and `time_t`, and 5.1's
//! and 5.2's `lua_Integer` (`ptrdiff_t`) is 32 bits too.

use crate::version::LuaVersion;

/// `long`.
#[cfg(target_pointer_width = "64")]
pub(crate) type CLong = i64;
/// `long`.
#[cfg(target_pointer_width = "32")]
pub(crate) type CLong = i32;

/// `time_t` (glibc's on i386 is 32 bits).
#[cfg(target_pointer_width = "64")]
pub(crate) type CTime = i64;
/// `time_t` (glibc's on i386 is 32 bits).
#[cfg(target_pointer_width = "32")]
pub(crate) type CTime = i32;

/// `(int32_t)d` as SSE2's `cvttsd2si` makes it: truncated toward zero,
/// and outside the range (or NaN) the "integer indefinite" `INT32_MIN`.
#[cfg(target_pointer_width = "32")]
#[inline]
pub(crate) fn cvtt_i32(d: f64) -> i32 {
    if d > -2_147_483_649.0 && d < 2_147_483_648.0 {
        d as i32
    } else {
        i32::MIN
    }
}

/// 5.2's `LUA_IEEE754TRICK`: add 1.5 * 2^52 and keep the low word of the
/// double, which rounds to nearest (ties to even) and wraps modulo 2^32.
#[inline]
pub(crate) fn ieee_trick(d: f64) -> u32 {
    (d + 6_755_399_441_055_744.0).to_bits() as u32
}

/// 5.1's and 5.2's `lua_number2integer`: a plain cast on a 64-bit target.
#[cfg(target_pointer_width = "64")]
#[inline(always)]
pub(crate) fn number2integer(_v: LuaVersion, d: f64) -> i64 {
    d as i64
}

/// 5.1's and 5.2's `lua_number2integer` on i386: 5.1 casts to its 32-bit
/// `lua_Integer`, 5.2 uses its `LUA_IEEE754TRICK` (with `LUA_IEEELL`).
#[cfg(target_pointer_width = "32")]
#[inline]
pub(crate) fn number2integer(v: LuaVersion, d: f64) -> i64 {
    if v == LuaVersion::Lua51 {
        i64::from(cvtt_i32(d))
    } else {
        i64::from(ieee_trick(d) as i32)
    }
}

/// `(long)d`.
#[cfg(target_pointer_width = "64")]
#[inline(always)]
pub(crate) fn to_long(d: f64) -> CLong {
    d as i64
}

/// `(long)d`: `cvttsd2si`.
#[cfg(target_pointer_width = "32")]
#[inline]
pub(crate) fn to_long(d: f64) -> CLong {
    cvtt_i32(d)
}

/// `(unsigned long)d`.
#[cfg(target_pointer_width = "64")]
#[inline(always)]
pub(crate) fn to_ulong(d: f64) -> u64 {
    d as u64
}

/// `(unsigned long)d` as gcc converts it with SSE2: below 2^31 through
/// `cvttsd2si`, from 2^31 on 2^31 less, the top bit flipped back (so
/// that the "integer indefinite" of a value past the range becomes 0).
#[cfg(target_pointer_width = "32")]
#[inline]
pub(crate) fn to_ulong(d: f64) -> u32 {
    if d >= 2_147_483_648.0 {
        cvtt_i32(d - 2_147_483_648.0) as u32 ^ 0x8000_0000
    } else {
        cvtt_i32(d) as u32
    }
}

/// `(int)d`.
#[cfg(target_pointer_width = "64")]
#[inline(always)]
pub(crate) fn to_int(d: f64) -> i32 {
    d as i32
}

/// `(int)d`: `cvttsd2si`.
#[cfg(target_pointer_width = "32")]
#[inline]
pub(crate) fn to_int(d: f64) -> i32 {
    cvtt_i32(d)
}

/// `(time_t)d`.
#[inline(always)]
pub(crate) fn to_time(d: f64) -> i64 {
    i64::from(to_long(d))
}

/// Whether `t` is a `time_t` (`(time_t)t == t`).
#[inline(always)]
pub(crate) fn is_time(t: i64) -> bool {
    CTime::try_from(t).is_ok()
}

/// 5.5's `MAX_SIZE`: the range of `size_t` when it is narrower than
/// `lua_Integer`, else `LUA_MAXINTEGER`.
pub(crate) const MAX_SIZE_55: u64 = if usize::BITS < 64 {
    usize::MAX as u64
} else {
    i64::MAX as u64
};

/// `offsetof(struct cD, u)` in `lstrlib.c`: the largest alignment of a
/// double, an integer and a pointer (4 on i386).
pub(crate) const MAXALIGN: u64 = {
    let (d, i, p) = (
        std::mem::align_of::<f64>(),
        std::mem::align_of::<i64>(),
        std::mem::align_of::<usize>(),
    );
    let m = if d > i { d } else { i };
    (if m > p { m } else { p }) as u64
};

/// `sizeof(long)`.
pub(crate) const SIZEOF_LONG: u64 = std::mem::size_of::<CLong>() as u64;

/// `sizeof(size_t)`.
pub(crate) const SIZEOF_SIZE_T: u64 = std::mem::size_of::<usize>() as u64;

/// `sizeof(size_t)`, as a binary chunk's header holds it.
pub(crate) const SIZE_T: u8 = SIZEOF_SIZE_T as u8;

/// 5.1's and 5.2's `lua_tointeger` of the number `n`: `lua_number2integer`
/// of the double it is in PUC.
#[inline]
pub(crate) fn tointeger52(v: LuaVersion, n: crate::numeric::Num) -> i64 {
    match n {
        #[cfg(target_pointer_width = "32")]
        crate::numeric::Num::Int(i) => number2integer(v, i as f64),
        #[cfg(target_pointer_width = "64")]
        crate::numeric::Num::Int(i) => i,
        crate::numeric::Num::Float(f) => number2integer(v, f),
    }
}

/// gcc for i386 evaluates a call's arguments from the last to the first:
/// where PUC checks two arguments inside one call (`fmod(luaL_checknumber(L,
/// 1), luaL_checknumber(L, 2))`), the last one's error comes first.
pub(crate) const ARGS_RTL: bool = cfg!(target_arch = "x86");

#[cfg(test)]
mod tests {
    use super::*;

    /// `(unsigned long)` and `(long)` of doubles, as gcc -m32 -msse2
    /// -mfpmath=sse compiles them (measured on i386).
    #[cfg(target_pointer_width = "32")]
    #[test]
    fn casts_are_i386_sse2s() {
        let cases: [(f64, u32, i32); 14] = [
            (1.5, 1, 1),
            (-1.5, 4294967295, -1),
            (2147483648.0, 2147483648, i32::MIN),
            (4294967295.0, 4294967295, i32::MIN),
            (4294967296.0, 0, i32::MIN),
            (-2147483649.0, 2147483648, i32::MIN),
            (1e15, 0, i32::MIN),
            (-1e15, 2147483648, i32::MIN),
            (1e300, 0, i32::MIN),
            (f64::INFINITY, 0, i32::MIN),
            (f64::NEG_INFINITY, 2147483648, i32::MIN),
            (f64::NAN, 2147483648, i32::MIN),
            (3e9, 3000000000, i32::MIN),
            (-3e9, 2147483648, i32::MIN),
        ];
        for (d, u, l) in cases {
            assert_eq!((to_ulong(d), to_long(d)), (u, l), "{d}");
        }
    }

    /// 5.2's trick rounds to nearest, ties to even, and wraps.
    #[test]
    fn the_ieee_trick_rounds_and_wraps() {
        assert_eq!(ieee_trick(3.5), 4);
        assert_eq!(ieee_trick(2.5), 2);
        assert_eq!(ieee_trick(-1.0), u32::MAX);
        assert_eq!(ieee_trick(4294967297.0), 1);
    }
}
