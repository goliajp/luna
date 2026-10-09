//! The C library's `errno`, as the process running PUC would have it.
//!
//! PUC reports a failed io or os call with `strerror(errno)` and the number,
//! read right after the call. Some failures set no `errno` (on Windows, a
//! write right after a read), and then what shows is whatever an earlier C
//! call left there: a `strtod` that overflowed in the lexer, a `pow` or `log`
//! out of range, a file that could not be opened. luna keeps the same value
//! and updates it where PUC's C calls would, following the C library of the
//! target: the Universal CRT on Windows, glibc elsewhere. Each rule below was
//! measured by calling the function with `errno` preset to 99.
//!
//! Like C's, the value is per thread.

use std::cell::Cell;

pub mod conv;
mod ucrt;
pub use ucrt::errno_of_win32;

/// `EDOM`
pub const EDOM: i32 = 33;
/// `ERANGE`
pub const ERANGE: i32 = 34;
/// `EINVAL`
pub const EINVAL: i32 = 22;

thread_local! {
    static ERRNO: Cell<i32> = const { Cell::new(0) };
    static WRITES: Cell<u64> = const { Cell::new(0) };
    static FOLD: Cell<Option<(u32, i32)>> = const { Cell::new(None) };
}

/// The current `errno`.
pub fn get() -> i32 {
    ERRNO.with(|e| e.get())
}

/// Set `errno`, as a C call that fails (or PUC's own `errno = 0`) does.
pub fn set(v: i32) {
    ERRNO.with(|e| e.set(v));
    WRITES.with(|w| w.set(w.get() + 1));
}

/// How many times `errno` has been set so far, which orders the writes.
pub fn writes() -> u64 {
    WRITES.with(|w| w.get())
}

// PUC folds a constant `^` or `%` while it parses, between the numerals the
// lexer converts (and anything a reader function does), while luna folds
// when it compiles, after the parse. The parser notes how many writes had
// happened when it made each such operation ([`writes`]); a fold's effect
// counts only when no write happened after that point, and of those the
// last operation made wins.

/// Start collecting the effects of the folds of one compilation.
pub(crate) fn begin_folds() {
    FOLD.with(|f| f.set(None));
}

/// A fold of the operation whose right operand is node `rhs`, made after
/// `stamp` writes, that leaves `effect`.
pub(crate) fn fold_effect(rhs: u32, stamp: u64, effect: Option<i32>) {
    let Some(v) = effect else { return };
    if stamp != writes() {
        return;
    }
    FOLD.with(|f| {
        if f.get().is_none_or(|(at, _)| at <= rhs) {
            f.set(Some((rhs, v)));
        }
    });
}

/// Apply what the folds of the compilation left.
pub(crate) fn end_folds() {
    if let Some((_, v)) = FOLD.with(|f| f.take()) {
        set(v);
    }
}

/// The C library whose rules apply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lib {
    /// The Universal CRT of Windows (MSVC).
    Ucrt,
    /// GNU libc.
    Glibc,
}

impl Lib {
    /// The C library of the target luna is built for.
    pub const HOST: Lib = if cfg!(windows) { Lib::Ucrt } else { Lib::Glibc };
}

fn is_tiny(x: f64) -> bool {
    x == 0.0 || x.is_subnormal()
}

/// What `strtod` leaves for a numeral that did not spell `inf` or `nan`
/// and converted to `x`; `nonzero` is whether its digits were not all
/// zeros and `exact` whether `x` is exactly the numeral's value.
pub fn strtod_errno(lib: Lib, x: f64, nonzero: bool, exact: bool) -> Option<i32> {
    let underflow = match lib {
        // only a result that is 0 although the numeral is not
        Lib::Ucrt => x == 0.0 && nonzero,
        // any inexact result in the subnormal range, or 0
        Lib::Glibc => nonzero && (x == 0.0 || (x.is_subnormal() && !exact)),
    };
    (x.is_infinite() || underflow).then_some(ERANGE)
}

/// Apply what `strtod` leaves after reading the decimal numeral `text`
/// (digits, an optional point and exponent, no sign) as `x`.
pub fn after_strtod_decimal(text: &str, x: f64) {
    let nonzero = text
        .bytes()
        .take_while(|c| !matches!(c, b'e' | b'E'))
        .any(|c| matches!(c, b'1'..=b'9'));
    let exact = !x.is_subnormal() || decimal_equals(text, x);
    apply(strtod_errno(Lib::HOST, x, nonzero, exact));
}

/// Whether the decimal numeral `text` is exactly `x` (finite, positive).
fn decimal_equals(text: &str, x: f64) -> bool {
    // both as (significant digits, decimal exponent of the first one)
    fn norm(mant: &str, exp: i64) -> (String, i64) {
        let point = mant.find('.').unwrap_or(mant.len()) as i64;
        let digits: String = mant.chars().filter(|c| *c != '.').collect();
        let lead = digits.len() - digits.trim_start_matches('0').len();
        let d = digits.trim_matches('0').to_string();
        (d, exp + point - lead as i64 - 1)
    }
    let (m, e) = match text.find(['e', 'E']) {
        Some(i) => (&text[..i], text[i + 1..].parse().unwrap_or(0)),
        None => (text, 0),
    };
    let exact = format!("{x:.1100e}");
    let (xm, xe) = exact.split_once('e').expect("exponent form");
    norm(m, e) == norm(xm, xe.parse().expect("an exponent"))
}

/// How a hexadecimal numeral is converted: by C99 `strtod`, or by Lua's
/// own reader, which ends in `ldexp`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HexConv {
    /// `strtod` (5.1; 5.3 and later where the C library is C99)
    Strtod,
    /// `lua_strx2number` of lobject.c (5.2; 5.3 and later on Windows)
    Own,
}

impl HexConv {
    /// What 5.3 and later use: their own reader where `LUA_USE_C89` is set,
    /// as luaconf.h sets it on Windows, `strtod` elsewhere.
    pub const LATER: HexConv = if cfg!(windows) {
        HexConv::Own
    } else {
        HexConv::Strtod
    };
}

/// What `ldexp(m, e)` leaves for a finite nonzero `m` and result `x`.
pub fn ldexp_errno(lib: Lib, m: f64, x: f64) -> Option<i32> {
    if !m.is_finite() || m == 0.0 {
        return None;
    }
    let underflow = lib == Lib::Glibc && x == 0.0;
    (x.is_infinite() || underflow).then_some(ERANGE)
}

/// The math functions PUC calls whose `errno` effect is set here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MathFn {
    /// `log`
    Log,
    /// `log2`
    Log2,
    /// `log10`
    Log10,
    /// `exp`
    Exp,
    /// `sqrt`
    Sqrt,
    /// `acos`
    Acos,
    /// `asin`
    Asin,
    /// `sin`
    Sin,
    /// `cos`
    Cos,
    /// `tan`
    Tan,
    /// `sinh`
    Sinh,
    /// `cosh`
    Cosh,
}

impl MathFn {
    /// Every function, indexed by the number compiled code passes for it.
    pub const ALL: [MathFn; 12] = [
        MathFn::Log,
        MathFn::Log2,
        MathFn::Log10,
        MathFn::Exp,
        MathFn::Sqrt,
        MathFn::Acos,
        MathFn::Asin,
        MathFn::Sin,
        MathFn::Cos,
        MathFn::Tan,
        MathFn::Sinh,
        MathFn::Cosh,
    ];

    /// The number compiled code passes for this function.
    pub fn index(self) -> i64 {
        Self::ALL.iter().position(|&f| f == self).expect("listed") as i64
    }

    /// The function's value at `x`, without the `errno` effect.
    pub fn eval(self, x: f64) -> f64 {
        match self {
            MathFn::Log => x.ln(),
            MathFn::Log2 => x.log2(),
            MathFn::Log10 => x.log10(),
            MathFn::Exp => x.exp(),
            MathFn::Sqrt => x.sqrt(),
            MathFn::Acos => x.acos(),
            MathFn::Asin => x.asin(),
            MathFn::Sin => x.sin(),
            MathFn::Cos => x.cos(),
            MathFn::Tan => x.tan(),
            MathFn::Sinh => x.sinh(),
            MathFn::Cosh => x.cosh(),
        }
    }
}

/// `f(x)` called as PUC calls it: the value, with `errno` updated.
pub fn math1(f: MathFn, x: f64) -> f64 {
    let r = f.eval(x);
    apply(math1_errno(Lib::HOST, f, x, r));
    r
}

/// C `pow(x, y)`, with `errno` updated.
pub fn pow(x: f64, y: f64) -> f64 {
    let r = x.powf(y);
    apply(pow_errno(Lib::HOST, x, y, r));
    r
}

/// C `fmod(x, y)` as PUC's build computes it, with `errno` updated.
pub fn fmod(x: f64, y: f64) -> f64 {
    apply(fmod_errno(x, y));
    crate::vm::exec::c_fmod(x, y)
}

/// `r`, the result of C `ldexp(m, e)`, with `errno` updated.
pub fn ldexp_applied(m: f64, r: f64) -> f64 {
    apply(ldexp_errno(Lib::HOST, m, r));
    r
}

/// What the one-argument function `f` leaves for argument `x` and result
/// `r`; `None` leaves `errno` as it was.
pub fn math1_errno(lib: Lib, f: MathFn, x: f64, r: f64) -> Option<i32> {
    if x.is_nan() {
        return None;
    }
    match f {
        MathFn::Log | MathFn::Log2 | MathFn::Log10 => {
            if x < 0.0 {
                Some(EDOM)
            } else if x == 0.0 {
                Some(ERANGE)
            } else {
                None
            }
        }
        MathFn::Exp => {
            if x.is_infinite() {
                None
            } else if r.is_infinite() || (lib == Lib::Glibc && is_tiny(r)) {
                Some(ERANGE)
            } else {
                None
            }
        }
        MathFn::Sqrt => (x < 0.0).then_some(EDOM),
        MathFn::Acos | MathFn::Asin => (x.abs() > 1.0).then_some(EDOM),
        MathFn::Sin | MathFn::Cos | MathFn::Tan => {
            (lib == Lib::Ucrt && x.is_infinite()).then_some(EDOM)
        }
        MathFn::Sinh | MathFn::Cosh => (!x.is_infinite() && r.is_infinite()).then_some(ERANGE),
    }
}

/// What `pow(x, y)` leaves for result `r`.
pub fn pow_errno(lib: Lib, x: f64, y: f64, r: f64) -> Option<i32> {
    if x.is_nan() || y.is_nan() || x.is_infinite() || y.is_infinite() {
        return None;
    }
    if x < 0.0 && y.fract() != 0.0 {
        return Some(EDOM);
    }
    if x == 0.0 {
        // a pole: only the Universal CRT reports it
        return (y < 0.0 && lib == Lib::Ucrt).then_some(ERANGE);
    }
    (r.is_infinite() || r == 0.0).then_some(ERANGE)
}

/// What `fmod(x, y)` leaves.
pub fn fmod_errno(x: f64, y: f64) -> Option<i32> {
    if x.is_nan() || y.is_nan() {
        return None;
    }
    (y == 0.0 || x.is_infinite()).then_some(EDOM)
}

/// Apply an effect.
pub fn apply(e: Option<i32>) {
    if let Some(v) = e {
        set(v);
    }
}

#[cfg(test)]
mod tests;
