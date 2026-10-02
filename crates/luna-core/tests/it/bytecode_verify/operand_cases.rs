//! Operands that name a constant, an upvalue or a nested function.

use super::*;

#[test]
fn constant_index_out_of_range() {
    refused(
        "return function() return 'k' end",
        |p| {
            let pc = find(p, Op::LoadK);
            set(p, pc, |i| with_bx(i, 999));
        },
        "constant 999 out of range",
    );
}

#[test]
fn upvalue_index_out_of_range() {
    refused(
        "local u = 1 return function() return u end",
        |p| {
            let pc = find(p, Op::GetUpval);
            set(p, pc, |i| with_b(i, 9));
        },
        "upvalue 9 out of range",
    );
}

#[test]
fn closure_index_out_of_range() {
    refused(
        "return function() return function() end end",
        |p| {
            let pc = find(p, Op::Closure);
            set(p, pc, |i| with_bx(i, 5));
        },
        "function 5 out of range",
    );
}

/// Index of the first constant of `p` that is not a string (tag 5).
fn non_string_const(p: &P) -> u32 {
    let mut r = Rd(&p.consts, 0);
    for k in 0..p.n_consts {
        match r.u8() {
            0..=2 => return k,
            3 | 4 => return k,
            5 => {
                r.bytes();
            }
            _ => {
                r.u32();
            }
        }
    }
    panic!("every constant is a string")
}

#[test]
fn field_key_constant_must_be_a_string() {
    for (src, op) in [
        ("return function(t) return t.x + 1.5 end", Op::GetField),
        ("return function(t) t.x = 1.5 end", Op::SetField),
        ("return function(o) return o:m(1.5) end", Op::SelfOp),
    ] {
        refused(
            src,
            |p| {
                let k = non_string_const(p);
                let pc = find(p, op);
                set(p, pc, |i| {
                    if op == Op::SetField {
                        with_b(i, k)
                    } else {
                        with_c(i, k)
                    }
                });
            },
            "is not a string",
        );
    }
}
