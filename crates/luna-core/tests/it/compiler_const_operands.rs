//! A constant operand of an arithmetic, bitwise or comparison operator is
//! encoded in the instruction instead of being loaded into a register
//! first: 5.4 / 5.5 take PUC's `ADDI`, `ADDK`, `EQI`... forms, 5.1–5.3 the
//! constant forms on whichever side PUC's `RK` operands put it.

use luna_core::compiler::compile_chunk;
use luna_core::frontend::parser::parse;
use luna_core::runtime::Heap;
use luna_core::version::LuaVersion;
use luna_core::vm::isa::{Inst, Op};

fn compile(version: LuaVersion, src: &str) -> Vec<Inst> {
    let ast = parse(src.as_bytes(), version).expect("parse");
    let mut heap = Heap::new();
    let proto = compile_chunk(&ast, version, b"=k", &mut heap).expect("compile");
    proto.code.to_vec()
}

fn ops(version: LuaVersion, src: &str) -> Vec<Op> {
    compile(version, src).iter().map(|i| i.op()).collect()
}

const ALL: [LuaVersion; 5] = [
    LuaVersion::Lua51,
    LuaVersion::Lua52,
    LuaVersion::Lua53,
    LuaVersion::Lua54,
    LuaVersion::Lua55,
];

#[test]
fn a_loop_body_has_no_constant_loads() {
    for v in ALL {
        let code = ops(
            v,
            "local s = 0 for i = 1, 100 do s = s + i % 7 end return s",
        );
        let body: Vec<Op> = code
            .iter()
            .skip_while(|&&op| !op.is_for_prep())
            .skip(1)
            .take_while(|&&op| !op.is_for_loop())
            .copied()
            .collect();
        assert_eq!(body, [Op::ModK, Op::Add], "{v:?}: {code:?}");
    }
}

/// The opcode, `k` and `C` of the instruction `expr` compiles to (the
/// first one that is not a load, a move or the vararg read).
fn form(v: LuaVersion, expr: &str) -> (Op, bool, u32) {
    let code = compile(v, &format!("local x = ... {expr}"));
    let i = code
        .iter()
        .find(|i| {
            !matches!(
                i.op(),
                Op::Vararg | Op::LoadI | Op::LoadF | Op::LoadK | Op::Move | Op::LoadNil
            )
        })
        .expect("an operation");
    assert!(
        !code.iter().any(
            |i| matches!(i.op(), Op::LoadI | Op::LoadF | Op::LoadK | Op::LoadNil) && i.a() > 0
        ),
        "{v:?} `{expr}` still loads its constant: {code:?}"
    );
    let c = if i.op().is_test() { i.c() } else { 0 };
    (i.op(), i.k(), c)
}

#[test]
fn each_operator_takes_its_constant_form() {
    // (expression, 5.1–5.3 form, 5.4+ form): opcode, `k`, `C` of a test
    let cases: &[(&str, (Op, bool, u32), (Op, bool, u32))] = &[
        ("return x + 1", (Op::AddK, false, 0), (Op::AddI, false, 0)),
        ("return 1 + x", (Op::AddK, true, 0), (Op::AddI, true, 0)),
        (
            "return x + 1000",
            (Op::AddK, false, 0),
            (Op::AddK, false, 0),
        ),
        ("return x + 0.5", (Op::AddK, false, 0), (Op::AddK, false, 0)),
        ("return x - 1", (Op::SubK, false, 0), (Op::SubI, false, 0)),
        ("return x - 0", (Op::SubK, false, 0), (Op::SubI, false, 0)),
        (
            "return x - 1000",
            (Op::SubK, false, 0),
            (Op::SubK, false, 0),
        ),
        ("return x * 2", (Op::MulK, false, 0), (Op::MulK, false, 0)),
        ("return 2 * x", (Op::MulK, true, 0), (Op::MulK, true, 0)),
        ("return x % 7", (Op::ModK, false, 0), (Op::ModK, false, 0)),
        ("return x ^ 2", (Op::PowK, false, 0), (Op::PowK, false, 0)),
        ("return x / 4", (Op::DivK, false, 0), (Op::DivK, false, 0)),
        ("return x == 3", (Op::EqK, true, 0), (Op::EqI, true, 0)),
        ("return x == 'a'", (Op::EqK, true, 0), (Op::EqK, true, 0)),
        ("return x == nil", (Op::EqK, true, 0), (Op::EqK, true, 0)),
        ("return x < 3", (Op::LtK, true, 0), (Op::LtI, true, 0)),
        ("return x <= 3", (Op::LeK, true, 0), (Op::LeI, true, 0)),
        ("return x > 3", (Op::LtK, true, 1), (Op::GtI, true, 0)),
        ("return x >= 3", (Op::LeK, true, 1), (Op::GeI, true, 0)),
        ("return 3 < x", (Op::LtK, true, 1), (Op::GtI, true, 0)),
        ("return 3 >= x", (Op::LeK, true, 0), (Op::LeI, true, 0)),
        ("return x < 2.0", (Op::LtK, true, 0), (Op::LtI, true, 1)),
    ];
    for v in ALL {
        for &(expr, classic, modern) in cases {
            let want = if v <= LuaVersion::Lua53 {
                classic
            } else {
                modern
            };
            assert_eq!(form(v, expr), want, "{v:?} `{expr}`");
        }
    }
    let bitwise: &[(&str, (Op, bool, u32), (Op, bool, u32))] = &[
        (
            "return x // 3",
            (Op::IDivK, false, 0),
            (Op::IDivK, false, 0),
        ),
        (
            "return x & 12",
            (Op::BAndK, false, 0),
            (Op::BAndK, false, 0),
        ),
        ("return 12 | x", (Op::BOrK, true, 0), (Op::BOrK, true, 0)),
        (
            "return x ~ 255",
            (Op::BXorK, false, 0),
            (Op::BXorK, false, 0),
        ),
        ("return x >> 2", (Op::ShrK, false, 0), (Op::ShrI, false, 0)),
        ("return x << 2", (Op::ShlK, false, 0), (Op::ShlI, false, 0)),
        ("return 2 << x", (Op::ShlK, true, 0), (Op::ShlI, true, 0)),
    ];
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for &(expr, classic, modern) in bitwise {
            let want = if v <= LuaVersion::Lua53 {
                classic
            } else {
                modern
            };
            assert_eq!(form(v, expr), want, "{v:?} `{expr}`");
        }
    }
}

#[test]
fn before_54_any_constant_is_an_operand_on_either_side() {
    let cases: &[(&str, (Op, bool, u32))] = &[
        ("return '1' + x", (Op::AddK, true, 0)),
        ("return x + '1'", (Op::AddK, false, 0)),
        ("return 1 - x", (Op::SubK, true, 0)),
        ("return 2 ^ x", (Op::PowK, true, 0)),
        ("return 5 % x", (Op::ModK, true, 0)),
        ("return x + nil", (Op::AddK, false, 0)),
        ("return x < 1e300", (Op::LtK, true, 0)),
        ("return 'a' <= x", (Op::LeK, true, 1)),
        ("return nil == x", (Op::EqK, true, 1)),
        ("return true < x", (Op::LtK, true, 1)),
        ("return 1 / 0", (Op::DivKK, false, 0)),
        ("return 1 < 2", (Op::LtKK, true, 0)),
        ("return nil == false", (Op::EqKK, true, 0)),
    ];
    for v in [LuaVersion::Lua51, LuaVersion::Lua52, LuaVersion::Lua53] {
        for &(expr, want) in cases {
            assert_eq!(form(v, expr), want, "{v:?} `{expr}`");
        }
    }
}

#[test]
fn a_float_immediate_is_marked_in_c() {
    let code = compile(LuaVersion::Lua54, "local x = ... return x < 2.0, x < 2");
    let flags: Vec<u32> = code
        .iter()
        .filter(|i| i.op() == Op::LtI)
        .map(|i| i.c())
        .collect();
    assert_eq!(flags, [1, 0], "{code:?}");
}
