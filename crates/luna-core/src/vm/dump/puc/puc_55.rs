//! PUC Lua 5.5 `.luac` → luna `Proto` translator.
//!
//! Reads the chunk format of `lua-5.5.1/src/lundump.c`; the code itself is
//! translated by [`super::modern`], which 5.5 shares with 5.4.
//!
//! Format details particular to 5.5:
//!
//! - **Varints** (`loadVarint`) are most-significant-group first with the
//!   high bit marking *continuation* — the opposite of 5.4.
//! - **Strings** are saved once: a later occurrence is size 0 followed by
//!   the 1-based index of the earlier one (index 0 is NULL).
//! - **Alignment**: the code vector, and `abslineinfo` when present, start
//!   at a multiple of 4 bytes from the beginning of the chunk.
//! - **Integers** are zig-zag varints; the function's source follows its
//!   nested functions.

use super::lower::{self, Lowered, RawLocVar, RawProto};
use super::modern::{self, Dialect, Kind};
use crate::runtime::Value;
use crate::runtime::function::{Proto, UpvalDesc};
use crate::runtime::heap::{Gc, Heap};
use crate::runtime::string::LuaStr;
use crate::vm::dump::reader::{Reader, read_puc_varint};
use crate::vm::isa::Op;

const DIALECT: &str = "PUC 5.5";

/// Header: signature, version, format, `LUAC_DATA`, then size-prefixed
/// samples of `int` (`LUAC_INT` = -0x5678), `Instruction` (0x12345678),
/// `lua_Integer` (-0x5678) and `lua_Number` (-370.5), all little-endian.
const HEADER: &[u8] = &[
    0x1b, b'L', b'u', b'a', 0x55, 0, // signature, version, format
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n', // LUAC_DATA
    4, 0x88, 0xA9, 0xFF, 0xFF, // int
    4, 0x78, 0x56, 0x34, 0x12, // Instruction
    8, 0x88, 0xA9, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, // lua_Integer
    8, 0, 0, 0, 0, 0, 0x28, 0x77, 0xC0, // lua_Number
];

/// `Proto.flag` bits (lobject.h): hidden vararg arguments, and a vararg
/// table built at entry.
const PF_VAHID: u8 = 1;
const PF_VATAB: u8 = 2;

/// Opcode numbers, lopcodes.h 5.5.1.
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
    Kind::ArithI, // SHLI
    Kind::ArithI, // SHRI
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
    Kind::GetVarg,
    Kind::ErrNNil,
    Kind::VarargPrep,
    Kind::ExtraArg,
];

const D55: Dialect = Dialect {
    name: DIALECT,
    ops: OPS,
    v55: true,
};

pub(super) fn undump_puc_55(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    check_header(bytes)?;
    let mut r = Reader::at(bytes, HEADER.len());
    let n_upvals = r.u8()? as usize;
    let mut strings = Vec::new();
    let raw = read_proto(&mut r, heap, &mut strings)?;
    if raw.upvals.len() != n_upvals {
        return Err(format!(
            "{DIALECT} chunk: main closure has {n_upvals} upvalues, its function {}",
            raw.upvals.len()
        ));
    }
    if r.pos() != bytes.len() {
        return Err(format!(
            "{DIALECT} chunk: {} trailing bytes",
            bytes.len() - r.pos()
        ));
    }
    lower::build(heap, raw, &translate)
}

fn translate(raw: &mut RawProto) -> Result<Lowered, String> {
    modern::translate(&D55, raw)
}

fn check_header(bytes: &[u8]) -> Result<(), String> {
    let Some(h) = bytes.get(..HEADER.len()) else {
        return Err(format!("{DIALECT} chunk: truncated header"));
    };
    match h.iter().zip(HEADER).position(|(a, b)| a != b) {
        None => Ok(()),
        Some(5) => Err(format!(
            "{DIALECT} chunk: unsupported format byte 0x{:02x}",
            h[5]
        )),
        Some(6..=11) => Err(format!("{DIALECT} chunk: corrupted LUAC_DATA")),
        Some(12..=16) => Err(format!(
            "{DIALECT} chunk: `int` is not a 4-byte little-endian int"
        )),
        Some(17..=21) => Err(format!(
            "{DIALECT} chunk: Instruction is not 4 bytes little-endian"
        )),
        Some(22..=30) => Err(format!(
            "{DIALECT} chunk: lua_Integer is not a 64-bit little-endian integer"
        )),
        Some(_) => Err(format!("{DIALECT} chunk: lua_Number is not an IEEE double")),
    }
}

/// `loadInt`: a varint that must fit a C `int`.
fn read_int(r: &mut Reader) -> Result<u32, String> {
    let v = read_puc_varint(r)?;
    u32::try_from(v)
        .ok()
        .filter(|&v| v <= i32::MAX as u32)
        .ok_or_else(|| format!("{DIALECT} chunk: integer overflow"))
}

/// An element count, checked against the bytes left (see `Reader::count`).
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, String> {
    let n = read_int(r)?;
    r.count(n as u64, min_size)
}

/// Skip `loadAlign` padding: offsets count from the start of the chunk.
fn align(r: &mut Reader, to: usize) -> Result<(), String> {
    let pad = (to - r.pos() % to) % to;
    r.take(pad)?;
    Ok(())
}

/// `loadString`: size 0 refers back to an earlier string (index 0: NULL);
/// otherwise the size is length + 1 and the bytes carry a trailing NUL.
fn read_string(
    r: &mut Reader,
    heap: &mut Heap,
    strings: &mut Vec<Gc<LuaStr>>,
) -> Result<Option<Gc<LuaStr>>, String> {
    let size = read_puc_varint(r)?;
    if size == 0 {
        let idx = read_puc_varint(r)?;
        if idx == 0 {
            return Ok(None);
        }
        return match strings.get(idx as usize - 1) {
            Some(&s) => Ok(Some(s)),
            None => Err(format!("{DIALECT} chunk: invalid string index {idx}")),
        };
    }
    let n = r.count(size, 1)?;
    let bytes = r.take(n)?;
    let s = heap.intern(&bytes[..n - 1]);
    strings.push(s);
    Ok(Some(s))
}

fn read_name(
    r: &mut Reader,
    heap: &mut Heap,
    strings: &mut Vec<Gc<LuaStr>>,
) -> Result<Box<str>, String> {
    Ok(match read_string(r, heap, strings)? {
        Some(s) => String::from_utf8_lossy(s.as_bytes()).into(),
        None => "".into(),
    })
}

fn read_const(
    r: &mut Reader,
    heap: &mut Heap,
    strings: &mut Vec<Gc<LuaStr>>,
) -> Result<Value, String> {
    // Tags are `makevariant(type, variant)` (lobject.h).
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(false),
        17 => Value::Bool(true),
        3 => {
            // zig-zag: even is non-negative, odd is the complement
            let cx = read_puc_varint(r)?;
            let half = (cx >> 1) as i64;
            Value::Int(if cx & 1 != 0 { !half } else { half })
        }
        19 => Value::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        4 | 20 => match read_string(r, heap, strings)? {
            Some(s) => Value::Str(s),
            None => return Err(format!("{DIALECT} chunk: NULL string constant")),
        },
        t => return Err(format!("{DIALECT} chunk: bad constant tag {t}")),
    })
}

/// `loadFunction` order: line_defined, last_line_defined, numparams, flag,
/// maxstacksize, code, constants, upvalues, protos, source, debug section.
fn read_proto(
    r: &mut Reader,
    heap: &mut Heap,
    strings: &mut Vec<Gc<LuaStr>>,
) -> Result<RawProto, String> {
    let line_defined = read_int(r)?;
    let last_line_defined = read_int(r)?;
    let num_params = r.u8()?;
    let flag = r.u8()?;
    let max_stack = r.u8()?;

    let n = read_count(r, 4)?;
    align(r, 4)?;
    let code = (0..n).map(|_| r.u32()).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 1)?;
    let consts = (0..n)
        .map(|_| read_const(r, heap, strings))
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
        .map(|_| read_proto(r, heap, strings))
        .collect::<Result<Vec<_>, _>>()?;
    // A stripped chunk has no source; errors then report "?".
    let source = match read_string(r, heap, strings)? {
        Some(s) => s,
        None => heap.intern(b"=?"),
    };

    let n = read_count(r, 1)?;
    let deltas = r.take(n)?.to_vec();
    let n = read_count(r, 8)?;
    let mut abs = Vec::with_capacity(n);
    if n > 0 {
        align(r, 4)?;
        for _ in 0..n {
            let pc = u32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"));
            let line = u32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"));
            abs.push((pc, line));
        }
    }
    let lines = lower::rle_lines(DIALECT, &deltas, &abs, line_defined, code.len())?;
    let n = read_count(r, 3)?;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        locvars.push(RawLocVar {
            name: read_name(r, heap, strings)?,
            start_pc: read_int(r)?,
            end_pc: read_int(r)?,
        });
    }
    // A non-zero count means "one name per upvalue".
    if read_int(r)? != 0 {
        for u in upvals.iter_mut() {
            u.name = read_name(r, heap, strings)?;
        }
    }

    Ok(RawProto {
        source,
        line_defined,
        last_line_defined,
        num_params,
        is_vararg: flag & (PF_VAHID | PF_VATAB) != 0,
        has_compat_vararg_arg: false,
        vararg_table: flag & PF_VATAB != 0,
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

    /// ivABC: `op:7 | A:8 | k:1 | vB:6 | vC:10`
    fn vabck(op: u32, a: u32, vb: u32, vc: u32, k: bool) -> u32 {
        op | (a << 7) | ((k as u32) << 15) | (vb << 16) | (vc << 22)
    }
    fn ax(op: u32, ax: u32) -> u32 {
        op | (ax << 7)
    }
    const SETLIST: u32 = 78;
    const EXTRAARG: u32 = 84;
    const RETURN0: u32 = 71;

    fn lower(code: Vec<u32>) -> Vec<Inst> {
        let mut heap = Heap::new();
        let mut raw = lower::test_proto(&mut heap, code, vec![], 8);
        translate(&mut raw).expect("translates").code
    }

    #[test]
    fn setlist_extraarg_extends_the_ten_bit_offset() {
        // 3 values stored after 2 * 1024 + 5 elements
        let code = lower(vec![
            vabck(SETLIST, 0, 3, 5, true),
            ax(EXTRAARG, 2),
            vabck(RETURN0, 0, 0, 0, false),
        ]);
        assert!(code[0].k());
        assert_eq!(code[0].b(), 3);
        assert_eq!((code[1].op(), code[1].ax()), (Op::ExtraArg, 2 * 1024 + 5));
    }
}
