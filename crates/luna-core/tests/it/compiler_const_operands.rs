//! A numeral operand of an arithmetic, bitwise or comparison operator is
//! encoded in the instruction (PUC 5.4 `ADDI`, `ADDK`, `EQI`, ...), in every
//! dialect, instead of being loaded into a register first.

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
            .skip_while(|&&op| op != Op::ForPrep)
            .skip(1)
            .take_while(|&&op| op != Op::ForLoop)
            .copied()
            .collect();
        assert_eq!(body, [Op::ModK, Op::Add], "{v:?}: {code:?}");
    }
}

#[test]
fn each_operator_takes_its_constant_form() {
    let cases: &[(&str, Op)] = &[
        ("return x + 1", Op::AddI),
        ("return 1 + x", Op::AddI),
        ("return x + 1000", Op::AddK),
        ("return x + 0.5", Op::AddK),
        ("return x - 1", Op::SubI),
        ("return x - 1000", Op::SubK),
        ("return x * 2", Op::MulK),
        ("return 2 * x", Op::MulK),
        ("return x % 7", Op::ModK),
        ("return x ^ 2", Op::PowK),
        ("return x / 4", Op::DivK),
        ("return x == 3", Op::EqI),
        ("return x == 'a'", Op::EqK),
        ("return x < 3", Op::LtI),
        ("return x <= 3", Op::LeI),
        ("return x > 3", Op::GtI),
        ("return x >= 3", Op::GeI),
        ("return 3 < x", Op::GtI),
        ("return 3 >= x", Op::LeI),
        ("return x < 2.0", Op::LtI),
    ];
    for v in ALL {
        for &(expr, want) in cases {
            // before 5.3 a numeral is a float: an immediate integer form
            // becomes the constant form
            let want = match (want, v <= LuaVersion::Lua52) {
                (Op::AddI, true) => Op::AddK,
                (Op::SubI, true) => Op::SubK,
                (w, _) => w,
            };
            let code = ops(v, &format!("local x = ... {expr}"));
            assert!(code.contains(&want), "{v:?} `{expr}`: {code:?}");
            assert!(
                !code
                    .iter()
                    .any(|&op| matches!(op, Op::LoadI | Op::LoadF | Op::LoadK)),
                "{v:?} `{expr}` still loads its constant: {code:?}"
            );
        }
    }
    let bitwise: &[(&str, Op)] = &[
        ("return x // 3", Op::IDivK),
        ("return x & 12", Op::BAndK),
        ("return 12 | x", Op::BOrK),
        ("return x ~ 255", Op::BXorK),
        ("return x >> 2", Op::ShrI),
        ("return x << 2", Op::ShlI),
    ];
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        for &(expr, want) in bitwise {
            let code = ops(v, &format!("local x = ... {expr}"));
            assert!(code.contains(&want), "{v:?} `{expr}`: {code:?}");
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
