//! PUC Lua 5.3 `.luac` → luna `Proto` translator.
//!
//! Reads the chunk format of `lua-5.3.6/src/lundump.c`; the code itself is
//! translated by [`super::classic`], which 5.3 shares with 5.2. Against 5.2
//! the format adds the integer subtype (`lua_Integer` constants), a
//! size-prefixed short-string encoding, a header that checks integer and
//! float representation, and the main closure's upvalue count before the
//! first function.

use super::classic::{self, Kind};
use super::lower::{self, Lowered, RawLocVar, RawProto};
use crate::runtime::Value;
use crate::runtime::function::{Proto, UpvalDesc};
use crate::runtime::heap::{Gc, Heap};
use crate::vm::dump::error::Bad;
use crate::vm::dump::header;
use crate::vm::dump::reader::Reader;
use crate::vm::isa::Op;

const DIALECT: &str = "PUC 5.3";

/// Header: signature, version, format, `LUAC_DATA`, the sizes of `int`,
/// `size_t`, `Instruction`, `lua_Integer` and `lua_Number`, then
/// `LUAC_INT` (0x5678) and `LUAC_NUM` (370.5), both little-endian.
const HEADER: &[u8] = &[
    0x1b, b'L', b'u', b'a', 0x53, 0, // signature, version, format
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n', // LUAC_DATA
    4, 8, 4, 8, 8, // int, size_t, Instruction, lua_Integer, lua_Number
    0x78, 0x56, 0, 0, 0, 0, 0, 0, // LUAC_INT
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40, // LUAC_NUM
];

/// Opcode numbers, lopcodes.h 5.3.6.
const OPS: &[Kind] = &[
    Kind::Move,
    Kind::LoadK,
    Kind::LoadKx,
    Kind::LoadBool,
    Kind::LoadNil,
    Kind::GetUpval,
    Kind::GetTabUp,
    Kind::GetTable,
    Kind::SetTabUp,
    Kind::SetUpval,
    Kind::SetTable,
    Kind::NewTable,
    Kind::SelfOp,
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
    Kind::Unary(Op::Unm),
    Kind::Unary(Op::BNot),
    Kind::Unary(Op::Not),
    Kind::Unary(Op::Len),
    Kind::Concat,
    Kind::Jmp,
    Kind::Eq,
    Kind::Lt,
    Kind::Le,
    Kind::Test,
    Kind::TestSet,
    Kind::Call,
    Kind::TailCall,
    Kind::Return,
    Kind::ForLoop,
    Kind::ForPrep,
    Kind::TForCall,
    Kind::TForLoop,
    Kind::SetList,
    Kind::Closure,
    Kind::Vararg,
    Kind::ExtraArg,
];

pub(super) fn undump_puc_53(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, Bad> {
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
    classic::translate(DIALECT, OPS, raw)
}

fn check_header(bytes: &[u8]) -> Result<(), Bad> {
    header::check(bytes, HEADER, header::LAYOUT_53)
}

fn read_int(r: &mut Reader) -> Result<u32, Bad> {
    let v = i32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"));
    u32::try_from(v).map_err(|_| Bad::IntOverflow)
}

/// An element count, checked against the bytes left (see `Reader::count`).
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, Bad> {
    let n = read_int(r)?;
    r.count(n as u64, min_size)
}

/// A size byte (`0xFF`: a `size_t` follows) holding length + 1; 0 is PUC's
/// NULL string.
fn read_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, Bad> {
    let b = r.u8()?;
    let size = if b == 0xFF {
        u64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))
    } else {
        b as u64
    };
    if size == 0 {
        return Ok(None);
    }
    let n = r.count(size - 1, 1)?;
    Ok(Some(r.take(n)?))
}

fn read_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, Bad> {
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(r.u8()? != 0),
        3 => Value::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        19 => Value::Int(i64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        4 | 20 => Value::Str(heap.intern(read_string(r)?.unwrap_or(b""))),
        _ => return Err(Bad::Constant),
    })
}

/// `LoadFunction` order: source, line_defined, last_line_defined,
/// numparams, is_vararg, maxstacksize, code, constants, upvalues, protos,
/// then the debug section (lineinfo, locvars, upvalue names).
fn read_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::string::LuaStr>>,
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
    let n = read_count(r, 2)?;
    let mut upvals = Vec::with_capacity(n);
    for _ in 0..n {
        upvals.push(UpvalDesc {
            in_stack: r.u8()? != 0,
            index: r.u8()?,
            name: "".into(),
            read_only: false,
        });
    }
    let n = read_count(r, 1)?;
    let protos = (0..n)
        .map(|_| read_proto(r, heap, Some(source)))
        .collect::<Result<Vec<_>, _>>()?;

    let n = read_count(r, 4)?;
    let lines = (0..n).map(|_| read_int(r)).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 9)?;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b""));
        locvars.push(RawLocVar {
            name: name.into(),
            start_pc: read_int(r)?,
            end_pc: read_int(r)?,
        });
    }
    let n = read_count(r, 1)?;
    if n > upvals.len() {
        return Err(format!(
            "{DIALECT} chunk: {n} upvalue names for {} upvalues",
            upvals.len()
        )
        .into());
    }
    for u in upvals.iter_mut().take(n) {
        u.name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b"")).into();
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

    /// `op:6 | A:8 | C:9 | B:9`
    fn abc(op: u32, a: u32, b: u32, c: u32) -> u32 {
        op | (a << 6) | (c << 14) | (b << 23)
    }
    fn asbx(op: u32, a: u32, sbx: i32) -> u32 {
        op | (a << 6) | (((sbx + 131071) as u32) << 14)
    }
    const EQ: u32 = 31;
    const JMP: u32 = 30;
    const LOADBOOL: u32 = 3;
    const RETURN: u32 = 38;

    fn lower(code: Vec<u32>) -> Vec<Inst> {
        let mut heap = Heap::new();
        let mut raw = lower::test_proto(&mut heap, code, vec![Value::Int(1)], 4);
        translate(&mut raw).expect("translates").code
    }

    #[test]
    fn a_closing_jump_guarded_by_a_comparison_stays_one_instruction() {
        // if r0 == K0 then goto <pc 3, closing r1..> end
        let code = lower(vec![
            abc(EQ, 0, 0, 0x100),
            asbx(JMP, 2, 1),
            abc(RETURN, 0, 1, 0),
            abc(RETURN, 0, 1, 0),
        ]);
        assert_eq!(code[0].op(), Op::EqK);
        assert_eq!(code[1].op(), Op::Jmp, "the skipped instruction is the jump");
        let tramp = (1 + 1 + code[1].sj()) as usize;
        assert_eq!((code[tramp].op(), code[tramp].a()), (Op::Close, 1));
        // the jump back is negative: add in signed arithmetic
        assert_eq!(tramp as i32 + 2 + code[tramp + 1].sj(), 3);
    }

    #[test]
    fn setlist_block_from_extraarg_becomes_an_element_offset() {
        const SETLIST: u32 = 43;
        const EXTRAARG: u32 = 46;
        // block 600 of 50 fields: the values go after element 29950
        let code = lower(vec![
            abc(SETLIST, 0, 2, 0),
            EXTRAARG | (600 << 6),
            abc(RETURN, 0, 1, 0),
        ]);
        assert_eq!(
            (code[0].op(), code[0].b(), code[0].k()),
            (Op::SetList, 2, true)
        );
        assert_eq!((code[1].op(), code[1].ax()), (Op::ExtraArg, 599 * 50));
    }

    #[test]
    fn loadbool_true_with_skip_jumps_over_the_next_instruction() {
        let code = lower(vec![
            abc(LOADBOOL, 0, 1, 1),
            abc(LOADBOOL, 0, 0, 0),
            abc(RETURN, 0, 2, 0),
        ]);
        assert_eq!(code[0].op(), Op::LoadTrue);
        assert_eq!((code[1].op(), 1 + 1 + code[1].sj()), (Op::Jmp, 3));
    }
}
