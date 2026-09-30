//! Register reads for the fast loop's arms, and the arithmetic arms built
//! on them.
//!
//! An arm that copies a register into a `Value` and matches on it makes the
//! compiler move the 16 bytes through a stack slot to get at the tag. These
//! helpers read the tag byte and the payload word in place instead (the
//! layout is fixed by `#[repr(C, u8)]`, see [`Value::tag_byte`]).

use crate::runtime::value::{Value, tag};

/// The tag byte of the value at `p`.
///
/// # Safety
/// `p` points at an initialised `Value`.
#[inline(always)]
pub(super) unsafe fn raw_tag(p: *const Value) -> u8 {
    // SAFETY: the caller's contract; the tag is the first byte
    unsafe { *(p as *const u8) }
}

/// The integer at `p`.
///
/// # Safety
/// `p` points at a `Value::Int`.
#[inline(always)]
pub(super) unsafe fn raw_int(p: *const Value) -> i64 {
    // SAFETY: the caller's contract; the payload is the second word
    unsafe { *(p as *const i64).add(1) }
}

/// The float at `p`.
///
/// # Safety
/// `p` points at a `Value::Float`.
#[inline(always)]
pub(super) unsafe fn raw_flt(p: *const Value) -> f64 {
    // SAFETY: as for `raw_int`
    unsafe { *(p as *const f64).add(1) }
}

/// The number at `p` as a float.
///
/// # Safety
/// `p` points at a `Value::Int` or a `Value::Float`, and `t` is its tag.
#[inline(always)]
pub(super) unsafe fn raw_num(p: *const Value, t: u8) -> f64 {
    // SAFETY: the caller's contract
    unsafe {
        if t == tag::INT {
            raw_int(p) as f64
        } else {
            raw_flt(p)
        }
    }
}

/// Overwrite the integer at `p`, keeping its tag.
///
/// # Safety
/// `p` points at a `Value::Int`.
#[inline(always)]
pub(super) unsafe fn put_int(p: *mut Value, v: i64) {
    // SAFETY: the caller's contract
    unsafe { *(p as *mut i64).add(1) = v }
}

/// Lua truth of the value at `p`: everything but `nil` and `false`.
///
/// # Safety
/// `p` points at an initialised `Value`.
#[inline(always)]
pub(super) unsafe fn raw_truthy(p: *const Value) -> bool {
    // SAFETY: the caller's contract; a `Bool`'s payload byte is initialised
    unsafe {
        let t = raw_tag(p);
        t > tag::BOOL || (t == tag::BOOL && *(p as *const u8).add(8) != 0)
    }
}

/// True when the tag is a number's.
#[inline(always)]
pub(super) fn is_num_tag(t: u8) -> bool {
    (t | 1) == tag::FLOAT
}

/// `R[A] := L op R` on the values at `$pl` and `$pr`: two integers and two
/// numbers are computed here (an arm yielding `None` falls through), the
/// rest by `$slow` with both operands. `true` when the arm finished the
/// operation.
macro_rules! arith_arm {
    ($regs:ident, $inst:ident, $pl:expr, $pr:expr,
     int($ia:ident, $ib:ident) => $iv:expr, float($fa:ident, $fb:ident) => $fv:expr,
     slow($l:ident, $r:ident) => $sv:expr) => {{
        use $crate::vm::exec::fast_arith::{is_num_tag, raw_int, raw_num, raw_tag};
        let (pl, pr): (*const Value, *const Value) = ($pl, $pr);
        // SAFETY: both point at initialised values, a register of the
        // running frame or a constant of its proto
        let (tl, tr) = unsafe { (raw_tag(pl), raw_tag(pr)) };
        let v: Option<Value> = if tl == tag::INT && tr == tag::INT {
            // SAFETY: both are integers
            let ($ia, $ib) = unsafe { (raw_int(pl), raw_int(pr)) };
            $iv
        } else if is_num_tag(tl) && is_num_tag(tr) {
            // SAFETY: both are numbers
            let ($fa, $fb) = unsafe { (raw_num(pl, tl), raw_num(pr, tr)) };
            $fv
        } else {
            None
        };
        match v {
            Some(v) => {
                // SAFETY: `$regs` is the running frame's register window
                unsafe { $regs.add($inst.a() as usize).write(v) };
                true
            }
            None => {
                // SAFETY: as above
                let ($l, $r) = unsafe { (*pl, *pr) };
                $sv?;
                false
            }
        }
    }};
}
pub(super) use arith_arm;

/// [`arith_arm`] with an integer immediate on the right.
macro_rules! arith_imm_arm {
    ($regs:ident, $inst:ident, $pl:expr, $im:expr,
     int($ia:ident, $ib:ident) => $iv:expr, float($fa:ident, $fb:ident) => $fv:expr,
     slow($l:ident, $r:ident) => $sv:expr) => {{
        use $crate::vm::exec::fast_arith::{raw_flt, raw_int, raw_tag};
        let pl: *const Value = $pl;
        let im: i64 = $im;
        // SAFETY: a register of the running frame
        let tl = unsafe { raw_tag(pl) };
        let v: Option<Value> = if tl == tag::INT {
            // SAFETY: an integer
            let ($ia, $ib) = (unsafe { raw_int(pl) }, im);
            $iv
        } else if tl == tag::FLOAT {
            // SAFETY: a float
            let ($fa, $fb) = (unsafe { raw_flt(pl) }, im as f64);
            $fv
        } else {
            None
        };
        match v {
            Some(v) => {
                // SAFETY: `$regs` is the running frame's register window
                unsafe { $regs.add($inst.a() as usize).write(v) };
                true
            }
            None => {
                // SAFETY: as above
                let ($l, $r) = (unsafe { *pl }, Value::Int(im));
                $sv?;
                false
            }
        }
    }};
}
pub(super) use arith_imm_arm;
