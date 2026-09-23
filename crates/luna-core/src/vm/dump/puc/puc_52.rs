//! PUC Lua 5.2 `.luac` → luna `Proto` translator.
//!
//! Reads the chunk format of `lua-5.2.4/src/lundump.c`; the code itself is
//! translated by [`super::classic`], which 5.2 shares with 5.3. 5.2 has a
//! real `_ENV` upvalue and float-only numbers, so its functions map onto
//! luna's without synthesis.

use super::classic::{self, Kind};
use super::lower::{self, Lowered, RawLocVar, RawProto};
use crate::runtime::Value;
use crate::runtime::function::{Proto, UpvalDesc};
use crate::runtime::heap::{Gc, Heap};
use crate::vm::dump::reader::Reader;
use crate::vm::isa::Op;

const DIALECT: &str = "PUC 5.2";

/// The C types whose sizes header bytes 7..=10 record.
const SIZE_NAMES: [&str; 4] = [
    "sizeof(int)",
    "sizeof(size_t)",
    "sizeof(Instruction)",
    "sizeof(lua_Number)",
];

/// Header: signature, version, format, endianness, the sizes of `int`,
/// `size_t`, `Instruction` and `lua_Number`, the integral flag, then
/// `LUAC_TAIL`.
const HEADER: &[u8] = &[
    0x1b, b'L', b'u', b'a', 0x52, 0, 1, 4, 8, 4, 8, 0, 0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
];

/// Opcode numbers, lopcodes.h 5.2.4.
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
    Kind::Arith(Op::Div),
    Kind::Arith(Op::Mod),
    Kind::Arith(Op::Pow),
    Kind::Unary(Op::Unm),
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

pub(super) fn undump(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    check_header(bytes)?;
    let mut r = Reader::at(bytes, HEADER.len());
    let raw = read_proto(&mut r, heap)?;
    if r.pos() != bytes.len() {
        return Err(format!(
            "{DIALECT} chunk: {} trailing bytes",
            bytes.len() - r.pos()
        ));
    }
    lower::build(heap, raw, &translate)
}

fn translate(raw: &mut RawProto) -> Result<Lowered, String> {
    classic::translate(DIALECT, OPS, raw)
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
        Some(6) => Err(format!("{DIALECT} chunk: only little-endian chunks load")),
        Some(11) => Err(format!(
            "{DIALECT} chunk: integral lua_Number builds are not supported"
        )),
        Some(i @ 7..=10) => Err(format!(
            "{DIALECT} chunk: {} is {}, luna needs {}",
            SIZE_NAMES[i - 7],
            h[i],
            HEADER[i]
        )),
        Some(i) if i < 11 => Err(format!("{DIALECT} chunk: bad signature or version")),
        Some(_) => Err(format!("{DIALECT} chunk: corrupted header tail")),
    }
}

fn read_int(r: &mut Reader) -> Result<u32, String> {
    let v = i32::from_le_bytes(r.take(4)?.try_into().expect("4 bytes"));
    u32::try_from(v).map_err(|_| format!("{DIALECT} chunk: negative count {v}"))
}

/// An element count, checked against the bytes left (see `Reader::count`).
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, String> {
    let n = read_int(r)?;
    r.count(n as u64, min_size)
}

/// `size_t` length then the bytes, including a trailing NUL that luna drops.
/// Length 0 is PUC's NULL string.
fn read_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, String> {
    let n = u64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"));
    if n == 0 {
        return Ok(None);
    }
    let n = r.count(n, 1)?;
    let s = r.take(n)?;
    Ok(Some(&s[..n - 1]))
}

fn read_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(r.u8()? != 0),
        3 => Value::Float(f64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"))),
        4 => Value::Str(heap.intern(read_string(r)?.unwrap_or(b""))),
        t => return Err(format!("{DIALECT} chunk: bad constant tag {t}")),
    })
}

/// `LoadFunction` order: line_defined, last_line_defined, numparams,
/// is_vararg, maxstacksize, code, constants, protos, upvalues, then the
/// debug section (source, lineinfo, locvars, upvalue names).
fn read_proto(r: &mut Reader, heap: &mut Heap) -> Result<RawProto, String> {
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
    let n = read_count(r, 1)?;
    let protos = (0..n)
        .map(|_| read_proto(r, heap))
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

    // A stripped chunk has no source; errors then report "?".
    let source = heap.intern(read_string(r)?.unwrap_or(b"=?"));
    let n = read_count(r, 4)?;
    let lines = (0..n).map(|_| read_int(r)).collect::<Result<Vec<_>, _>>()?;
    let n = read_count(r, 16)?;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = String::from_utf8_lossy(read_string(r)?.unwrap_or(b""));
        locvars.push(RawLocVar {
            name: name.into(),
            start_pc: read_int(r)?,
            end_pc: read_int(r)?,
        });
    }
    let n = read_count(r, 8)?;
    if n > upvals.len() {
        return Err(format!(
            "{DIALECT} chunk: {n} upvalue names for {} upvalues",
            upvals.len()
        ));
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
