//! PUC Lua 5.4 `.luac` → luna `Proto` translator.
//!
//! Reads the chunk format of `lua-5.4.9/src/lundump.c`; the code itself is
//! translated by [`super::modern`], which 5.4 shares with 5.5.
//!
//! Two format details are easy to get wrong:
//!
//! - **Varints** (`loadUnsigned`) are most-significant-group first, and a
//!   set high bit marks the *last* byte — the opposite convention to 5.5's
//!   `loadVarint`, so 5.4 keeps its own reader.
//! - **Line info** is a signed byte delta per instruction; the sentinel
//!   `-128` (`ABSLINEINFO`) means "take the absolute line from the next
//!   `abslineinfo` entry", not "subtract 128".

use super::lower::{self, Lowered, RawLocVar, RawProto};
use super::modern::{self, Dialect, Kind};
use crate::runtime::Value;
use crate::runtime::function::{Proto, UpvalDesc};
use crate::runtime::heap::{Gc, Heap};
use crate::runtime::string::LuaStr;
use crate::vm::dump::error::Bad;
use crate::vm::dump::header;
use crate::vm::dump::reader::Reader;
use crate::vm::isa::Op;

const DIALECT: &str = "PUC 5.4";

/// Header: signature, version, format, `LUAC_DATA`, the sizes of
/// `Instruction`, `lua_Integer` and `lua_Number`, then `LUAC_INT` (0x5678)
/// and `LUAC_NUM` (370.5), both little-endian.
const HEADER: &[u8] = &[
    0x1b, b'L', b'u', b'a', 0x54, 0, // signature, version, format
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n', // LUAC_DATA
    4, 8, 8, // Instruction, lua_Integer, lua_Number
    0x78, 0x56, 0, 0, 0, 0, 0, 0, // LUAC_INT
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40, // LUAC_NUM
];

/// Opcode numbers, lopcodes.h 5.4.9.
const OPS: &[Kind] = &[
    Kind::Move,
    Kind::LoadI,
    Kind::LoadF,
    Kind::LoadK,
    Kind::LoadKx,
    Kind::LoadFalse,
    Kind::LFalseSkip,
    Kind::LoadTrue,
    Kind::LoadNil,
    Kind::GetUpval,
    Kind::SetUpval,
    Kind::GetTabUp,
    Kind::GetTable,
    Kind::GetI,
    Kind::GetField,
    Kind::SetTabUp,
    Kind::SetTable,
    Kind::SetI,
    Kind::SetField,
    Kind::NewTable,
    Kind::SelfOp,
    Kind::ArithI, // ADDI
    Kind::ArithK, // ADDK
    Kind::ArithK, // SUBK
    Kind::ArithK, // MULK
    Kind::ArithK, // MODK
    Kind::ArithK, // POWK
    Kind::ArithK, // DIVK
    Kind::ArithK, // IDIVK
    Kind::ArithK, // BANDK
    Kind::ArithK, // BORK
    Kind::ArithK, // BXORK
    Kind::ArithI, // SHRI
    Kind::ArithI, // SHLI
    Kind::Arith(Op::Add),
    Kind::Arith(Op::Sub),
    Kind::Arith(Op::Mul),
    Kind::Arith(Op::Mod),
    Kind::Arith(Op::Pow),
    Kind::Arith(Op::Div),
    Kind::Arith(Op::IDiv),
    Kind::Arith(Op::BAnd),
    Kind::Arith(Op::BOr),
    Kind::Arith(Op::BXor),
    Kind::Arith(Op::Shl),
    Kind::Arith(Op::Shr),
    Kind::MmBin,
    Kind::MmBinI,
    Kind::MmBinK,
    Kind::Unary(Op::Unm),
    Kind::Unary(Op::BNot),
    Kind::Unary(Op::Not),
    Kind::Unary(Op::Len),
    Kind::Concat,
    Kind::Close,
    Kind::Tbc,
    Kind::Jmp,
    Kind::Eq,
    Kind::Lt,
    Kind::Le,
    Kind::EqK,
    Kind::EqI,
    Kind::LtI,
    Kind::LeI,
    Kind::GtI,
    Kind::GeI,
    Kind::Test,
    Kind::TestSet,
    Kind::Call,
    Kind::TailCall,
    Kind::Return,
    Kind::Return0,
    Kind::Return1,
    Kind::ForLoop,
    Kind::ForPrep,
    Kind::TForPrep,
    Kind::TForCall,
    Kind::TForLoop,
    Kind::SetList,
    Kind::Closure,
    Kind::Vararg,
    Kind::VarargPrep,
    Kind::ExtraArg,
];

const D54: Dialect = Dialect {
    name: DIALECT,
    ops: OPS,
    v55: false,
};

pub(super) fn undump(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, Bad> {
    check_header(bytes)?;
    let mut r = Reader::at(bytes, HEADER.len());
    // The main closure's upvalue count; its function repeats it.
    r.u8()?;
    let raw = read_proto(&mut r, heap, None)?;
    if r.pos() != bytes.len() {
        return Err(format!("{DIALECT} chunk: {} trailing bytes", bytes.len() - r.pos()).into());
    }
    Ok(lower::build(heap, raw, &translate)?)
}

fn translate(raw: &mut RawProto) -> Result<Lowered, String> {
    modern::translate(&D54, raw)
}

fn check_header(bytes: &[u8]) -> Result<(), Bad> {
    header::check(bytes, HEADER, header::LAYOUT_54)
}

/// `loadUnsigned`: 7-bit groups, most significant first, the last byte
/// flagged by its high bit.
fn read_varint(r: &mut Reader) -> Result<u64, Bad> {
    let mut x: u64 = 0;
    loop {
        let b = r.u8()?;
        if x >> 57 != 0 {
            return Err(Bad::IntOverflow);
        }
        x = (x << 7) | (b & 0x7F) as u64;
        if b & 0x80 != 0 {
            return Ok(x);
        }
    }
}

/// `loadInt`: a varint that must fit a C `int`.
fn read_int(r: &mut Reader) -> Result<u32, Bad> {
    let v = read_varint(r)?;
    u32::try_from(v)
        .ok()
        .filter(|&v| v <= i32::MAX as u32)
        .ok_or(Bad::IntOverflow)
}

/// An element count, checked against the bytes left (see `Reader::count`).
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, Bad> {
    let n = read_int(r)?;
    r.count(n as u64, min_size)
}

/// A varint holding length + 1; 0 is PUC's NULL string.
fn read_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, Bad> {
    let size = read_varint(r)?;
    if size == 0 {
        return Ok(None);
    }
    let n = r.count(size - 1, 1)?;
    Ok(Some(r.take(n)?))
}

fn read_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, Bad> {
    // Tags are `makevariant(type, variant)` (lobject.h).
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(false),
        17 => Value::Bool(true),
        3 => Value::Int(i64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        19 => Value::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        4 | 20 => match read_string(r)? {
            Some(s) => Value::Str(heap.intern(s)),
            None => return Err(format!("{DIALECT} chunk: NULL string constant").into()),
        },
        _ => return Err(Bad::Constant),
    })
}

/// `loadFunction` order: source, line_defined, last_line_defined,
/// numparams, is_vararg, maxstacksize, code, constants, upvalues, protos,
/// then the debug section.
fn read_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<LuaStr>>,
) -> Result<RawProto, Bad> {
    // A nested function's source is dumped as NULL when it equals its
    // parent's; a stripped main chunk has none at all.
    let source = match (read_string(r)?, parent_source) {
        (Some(s), _) => heap.intern(s),
        (None, Some(p)) => p,
        (None, None) => heap.intern(b"=?"),
    };
    let line_defined = read_int(r)?;
    let last_line_defined = read_int(r)?;
    let num_params = r.u8()?;
    let is_vararg = r.u8()? != 0;
    let max_stack = r.u8()?;

    let n = read_count(r, 4)?;
    let code = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 1)?;
    let consts = (0..n)
        .map(|_| read_const(r, heap))
        .collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 3)?;
    let mut upvals = Vec::with_capacity(n);
    for _ in 0..n {
        let in_stack = r.u8()? != 0;
        let index = r.u8()?;
        // kind: 0 regular, 1 <const>, 2 <close>, 3 compile-time constant
        let kind = r.u8()?;
        upvals.push(UpvalDesc {
            in_stack,
            index,
            name: "".into(),
            read_only: kind == 1 || kind == 3,
        });
    }
    let n = read_count(r, 1)?;
    let protos = (0..n)
        .map(|_| read_proto(r, heap, Some(source)))
        .collect::<Result<Vec<_>, _>>()?;

    let n = read_count(r, 1)?;
    let deltas = r.take(n)?.to_vec();
    let n = read_count(r, 2)?;
    let mut abs = Vec::with_capacity(n);
    for _ in 0..n {
        abs.push((read_int(r)?, read_int(r)?));
    }
    let lines = lower::rle_lines(DIALECT, &deltas, &abs, line_defined, code.len())?;
    let n = read_count(r, 3)?;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b""));
        locvars.push(RawLocVar {
            name: name.into(),
            start_pc: read_int(r)?,
            end_pc: read_int(r)?,
        });
    }
    // A non-zero count means "one name per upvalue".
    if read_int(r)? != 0 {
        for u in upvals.iter_mut() {
            u.name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b"")).into();
        }
    }

    Ok(RawProto {
        source,
        line_defined,
        last_line_defined,
        num_params,
        is_vararg,
        has_compat_vararg_arg: false,
        vararg_table: false,
        max_stack,
        code,
        consts,
        upvals,
        protos,
        lines,
        locvars,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::isa::Inst;

    /// `op:7 | A:8 | k:1 | B:8 | C:8`
    fn abck(op: u32, a: u32, b: u32, c: u32, k: bool) -> u32 {
        op | (a << 7) | ((k as u32) << 15) | (b << 16) | (c << 24)
    }
    const ADDI: u32 = 21;
    const ADDK: u32 = 22;
    const MMBINI: u32 = 47;
    const MMBINK: u32 = 48;
    const EQI: u32 = 61;
    const GTI: u32 = 64;
    const RETURN0: u32 = 71;
    const TM_ADD: u32 = 6;
    const TM_SUB: u32 = 7;

    fn lower(code: Vec<u32>, consts: Vec<Value>) -> Vec<Inst> {
        let mut heap = Heap::new();
        let mut raw = lower::test_proto(&mut heap, code, consts, 4);
        translate(&mut raw).expect("translates").code
    }

    #[test]
    fn immediate_arithmetic_takes_operator_and_operand_from_its_mmbin() {
        // `x - 1` compiles to ADDI x -1 with the original `- 1` on the MMBINI.
        let code = lower(
            vec![
                abck(ADDI, 0, 1, 127 - 1, false),
                abck(MMBINI, 1, 127 + 1, TM_SUB, false),
                abck(RETURN0, 0, 0, 0, false),
            ],
            vec![],
        );
        assert_eq!((code[0].op(), code[0].sbx()), (Op::LoadI, 1));
        let t = code[0].a();
        assert_eq!(
            (code[1].op(), code[1].a(), code[1].b(), code[1].c()),
            (Op::Sub, 0, 1, t)
        );
    }

    #[test]
    fn subtracting_an_immediate_zero_stays_an_addition() {
        // `x - 0` compiles to ADDI x 0 and runs as `x + 0`: `-0.0 - 0` is 0.0.
        let code = lower(
            vec![
                abck(ADDI, 0, 1, 127, false),
                abck(MMBINI, 1, 127, TM_SUB, false),
                abck(RETURN0, 0, 0, 0, false),
            ],
            vec![],
        );
        let t = code[0].a();
        assert_eq!(
            (code[1].op(), code[1].b(), code[1].c(), code[1].k()),
            (Op::Add, 1, t, true)
        );
        assert_eq!(code[1].source_op(), Op::Sub);
    }

    #[test]
    fn a_flipped_constant_operand_stays_on_the_left() {
        // `2.5 + x`: ADDK with the constant swapped to the right, MMBINK k=1.
        let code = lower(
            vec![
                abck(ADDK, 0, 1, 0, false),
                abck(MMBINK, 1, 0, TM_ADD, true),
                abck(RETURN0, 0, 0, 0, false),
            ],
            vec![Value::Float(2.5)],
        );
        let t = code[0].a();
        assert_eq!(code[0].op(), Op::LoadK);
        assert_eq!((code[1].op(), code[1].b(), code[1].c()), (Op::Add, t, 1));
    }

    #[test]
    fn immediate_comparisons_keep_float_literals_and_operand_order() {
        let code = lower(
            vec![
                abck(EQI, 0, 127 + 1, 1, true),
                abck(GTI, 0, 127 + 2, 0, false),
                abck(RETURN0, 0, 0, 0, false),
            ],
            vec![],
        );
        assert_eq!(code[0].op(), Op::LoadF, "C=1: the literal was 1.0");
        assert_eq!((code[1].op(), code[1].a(), code[1].k()), (Op::Eq, 0, true));
        assert_eq!(code[2].op(), Op::LoadI);
        let t = code[2].a();
        assert_eq!(
            (code[3].op(), code[3].a(), code[3].b()),
            (Op::Lt, t, 0),
            "x > 2 is 2 < x"
        );
    }
}
