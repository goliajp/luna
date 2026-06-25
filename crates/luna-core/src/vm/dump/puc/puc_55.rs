//! PUC Lua 5.5 `.luac` → luna `Proto` translator.
//!
//! Wave 2 of Phase LB. Reads a stock `luac5.5` output (header byte
//! `0x55`) and produces a `Gc<Proto>` ready for `Vm::call_value` on its
//! wrapping closure.
//!
//! Format reference: `lua-5.5.0/src/lundump.c`, `ldump.c`,
//! `lopcodes.h`. luna's opcode set is documented in `vm/isa.rs` as
//! "follows lopcodes.h (v5.5.0) with deliberate v1 trims" — most ops
//! translate 1:1, the exceptions are listed in the per-op match below.
//!
//! 0-dep contract: this file uses only the shared `super::super::reader`
//! primitives (`Reader`, `read_puc_varint`) and stdlib — no
//! `byteorder` / `leb128` crates per
//! `.dev/rfcs/v1.3-audit-puc-luac-formats.md` §"Cross-dialect risks".

use super::super::reader::{Reader, read_puc_varint};
use crate::runtime::Value;
use crate::runtime::function::{JitProtoState, LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::vm::isa::{Inst, Op};

// ---- PUC 5.5 header constants (from `lua-5.5.0/src/lua.h`, `lundump.h`) ----

/// `\x1bLua` — same across every PUC dialect.
const LUA_SIGNATURE: &[u8] = b"\x1bLua";
/// `LUAC_VERSION = LUA_VERSION_MAJOR_N * 16 + LUA_VERSION_MINOR_N = 5*16+5`.
const LUAC_VERSION: u8 = 0x55;
/// `LUAC_FORMAT = 0` — official format.
const LUAC_FORMAT: u8 = 0x00;
/// `LUAC_DATA` — 6-byte EOL / EOF check.
const LUAC_DATA: &[u8] = b"\x19\x93\r\n\x1a\n";
/// `LUAC_INT = -0x5678` — integer endianness probe.
const LUAC_INT_EXPECTED: i64 = -0x5678;
/// `LUAC_INST = 0x12345678` — instruction-size endianness probe.
const LUAC_INST_EXPECTED: u32 = 0x12345678;
/// `LUAC_NUM = cast_num(-370.5)` — number-format probe.
const LUAC_NUM_EXPECTED: f64 = -370.5;

// ---- PUC 5.5 constant-pool tag bytes (from `lobject.h` makevariant) ----

const LUA_VNIL: u8 = 0;
const LUA_VFALSE: u8 = 1;
const LUA_VTRUE: u8 = 1 | (1 << 4); // 17
const LUA_VNUMINT: u8 = 3;
const LUA_VNUMFLT: u8 = 3 | (1 << 4); // 19
const LUA_VSHRSTR: u8 = 4;
const LUA_VLNGSTR: u8 = 4 | (1 << 4); // 20

// ---- PUC 5.5 Proto.flag bits (from `lobject.h`) ----

const PF_VAHID: u8 = 1; // hidden vararg arguments
const PF_VATAB: u8 = 2; // explicit vararg table

// ---- PUC 5.5 RLE lineinfo sentinel (from `ldebug.h`) ----

/// `ABSLINEINFO = -0x80` (cast to i8): `lineinfo[pc]` carries this when
/// the delta from the previous instruction overflows i8 — the absolute
/// line is read from the next entry in `abslineinfo[]`.
const ABSLINEINFO: i8 = -0x80;

// ---- PUC 5.5 opcode numeric IDs (order from `lopcodes.h`'s `OpCode` enum) ----

#[allow(non_camel_case_types, dead_code, clippy::upper_case_acronyms)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
enum PucOp {
    MOVE = 0,
    LOADI = 1,
    LOADF = 2,
    LOADK = 3,
    LOADKX = 4,
    LOADFALSE = 5,
    LFALSESKIP = 6,
    LOADTRUE = 7,
    LOADNIL = 8,
    GETUPVAL = 9,
    SETUPVAL = 10,
    GETTABUP = 11,
    GETTABLE = 12,
    GETI = 13,
    GETFIELD = 14,
    SETTABUP = 15,
    SETTABLE = 16,
    SETI = 17,
    SETFIELD = 18,
    NEWTABLE = 19,
    SELF = 20,
    ADDI = 21,
    ADDK = 22,
    SUBK = 23,
    MULK = 24,
    MODK = 25,
    POWK = 26,
    DIVK = 27,
    IDIVK = 28,
    BANDK = 29,
    BORK = 30,
    BXORK = 31,
    SHLI = 32,
    SHRI = 33,
    ADD = 34,
    SUB = 35,
    MUL = 36,
    MOD = 37,
    POW = 38,
    DIV = 39,
    IDIV = 40,
    BAND = 41,
    BOR = 42,
    BXOR = 43,
    SHL = 44,
    SHR = 45,
    MMBIN = 46,
    MMBINI = 47,
    MMBINK = 48,
    UNM = 49,
    BNOT = 50,
    NOT = 51,
    LEN = 52,
    CONCAT = 53,
    CLOSE = 54,
    TBC = 55,
    JMP = 56,
    EQ = 57,
    LT = 58,
    LE = 59,
    EQK = 60,
    EQI = 61,
    LTI = 62,
    LEI = 63,
    GTI = 64,
    GEI = 65,
    TEST = 66,
    TESTSET = 67,
    CALL = 68,
    TAILCALL = 69,
    RETURN = 70,
    RETURN0 = 71,
    RETURN1 = 72,
    FORLOOP = 73,
    FORPREP = 74,
    TFORPREP = 75,
    TFORCALL = 76,
    TFORLOOP = 77,
    SETLIST = 78,
    CLOSURE = 79,
    VARARG = 80,
    GETVARG = 81,
    ERRNNIL = 82,
    VARARGPREP = 83,
    EXTRAARG = 84,
}

impl PucOp {
    fn from_byte(b: u8) -> Result<PucOp, String> {
        if b <= PucOp::EXTRAARG as u8 {
            // SAFETY: PucOp is repr(u8) and dense from 0..=EXTRAARG.
            Ok(unsafe { std::mem::transmute::<u8, PucOp>(b) })
        } else {
            Err(format!("unknown PUC 5.5 opcode 0x{b:02x}"))
        }
    }
}

// ---- PUC 5.5 instruction field accessors ----
//
// Layout (from `lopcodes.h`) matches luna's own iABC byte-for-byte
// except for the `ivABC` mode used by `OP_NEWTABLE` / `OP_SETLIST`:
//
//   iABC   :   Op(7) | A(8) | k(1) | B(8)  | C(8)
//   ivABC  :   Op(7) | A(8) | k(1) | vB(6) | vC(10)
//   iABx   :   Op(7) | A(8) | Bx(17)
//   iAsBx  :   Op(7) | A(8) | sBx(17)  (excess-encoded, K = MAXARG_Bx/2)
//   iAx    :   Op(7) | Ax(25)
//   isJ    :   Op(7) | sJ(25)          (excess-encoded, K = MAXARG_sJ/2)

const POS_A: u32 = 7;
const POS_K: u32 = 15;
const POS_B: u32 = 16;
const POS_C: u32 = 24;
const POS_VB: u32 = 16;
const POS_VC: u32 = 22;
const POS_BX: u32 = 15;
const POS_AX: u32 = 7;
const POS_SJ: u32 = 7;
const SIZE_BX: u32 = 17;
const SIZE_AX: u32 = 25;
const SIZE_SJ: u32 = 25;
const MAXARG_BX: u32 = (1 << SIZE_BX) - 1;
const MAXARG_AX: u32 = (1 << SIZE_AX) - 1;
const MAXARG_SJ: u32 = (1 << SIZE_SJ) - 1;
const OFFSET_SBX: i32 = (MAXARG_BX >> 1) as i32;
const OFFSET_SJ: i32 = (MAXARG_SJ >> 1) as i32;
const OFFSET_SC: i32 = 0xFF >> 1; // SIZE_C = 8 → offset for sC excess-encoding

#[inline]
fn f_a(i: u32) -> u32 {
    (i >> POS_A) & 0xFF
}
#[inline]
fn f_k(i: u32) -> bool {
    ((i >> POS_K) & 1) != 0
}
#[inline]
fn f_b(i: u32) -> u32 {
    (i >> POS_B) & 0xFF
}
#[inline]
fn f_c(i: u32) -> u32 {
    (i >> POS_C) & 0xFF
}
#[inline]
fn f_sb(i: u32) -> i32 {
    f_b(i) as i32 - OFFSET_SC
}
#[inline]
fn f_sc(i: u32) -> i32 {
    f_c(i) as i32 - OFFSET_SC
}
#[inline]
fn f_vb(i: u32) -> u32 {
    (i >> POS_VB) & 0x3F
}
#[inline]
fn f_vc(i: u32) -> u32 {
    (i >> POS_VC) & 0x3FF
}
#[inline]
fn f_bx(i: u32) -> u32 {
    (i >> POS_BX) & MAXARG_BX
}
#[inline]
fn f_sbx(i: u32) -> i32 {
    f_bx(i) as i32 - OFFSET_SBX
}
#[inline]
fn f_ax(i: u32) -> u32 {
    (i >> POS_AX) & MAXARG_AX
}
#[inline]
fn f_sj(i: u32) -> i32 {
    f_ax(i) as i32 - OFFSET_SJ
}

// ---- header / value helpers ----

fn check_literal(r: &mut Reader, expected: &[u8], msg: &str) -> Result<(), String> {
    let got = r.take(expected.len())?;
    if got != expected {
        return Err(format!("bad PUC 5.5 binary chunk: {msg}"));
    }
    Ok(())
}

fn load_byte(r: &mut Reader) -> Result<u8, String> {
    r.u8()
}

fn load_unsigned(r: &mut Reader) -> Result<u64, String> {
    read_puc_varint(r)
}

fn load_int(r: &mut Reader) -> Result<i32, String> {
    let v = read_puc_varint(r)?;
    if v > i32::MAX as u64 {
        return Err("PUC 5.5 int overflows i32".to_string());
    }
    Ok(v as i32)
}

fn load_size(r: &mut Reader) -> Result<usize, String> {
    let v = read_puc_varint(r)?;
    if v > usize::MAX as u64 {
        return Err("PUC 5.5 size overflows usize".to_string());
    }
    Ok(v as usize)
}

/// PUC 5.5 `loadInteger` zig-zag decode:
///   `(cx & 1) == 0` → +(cx >> 1)
///   `(cx & 1) != 0` → -((cx >> 1) + 1) = `~(cx >> 1)`
fn load_integer(r: &mut Reader) -> Result<i64, String> {
    let cx = read_puc_varint(r)?;
    let half = (cx >> 1) as i64;
    if cx & 1 != 0 {
        Ok(!half) // ~half
    } else {
        Ok(half)
    }
}

fn load_number(r: &mut Reader) -> Result<f64, String> {
    let bytes: [u8; 8] = r.take(8)?.try_into().unwrap();
    Ok(f64::from_le_bytes(bytes))
}

/// PUC 5.5 strings have two shapes (from `lundump.c::loadString`):
///   - `size == 0`: a back-reference. The next varint is the index in
///     `S->h` of a previously-loaded string (or `0` = NULL).
///   - `size >= 1`: a real string of `size - 1` bytes follows, and the
///     loader appends it to `S->h` for future back-references.
///
/// luna interns all strings flat (no PUC `nstr` table to maintain), but
/// we still have to play the back-reference game when decoding because
/// PUC's dumper emits them. We keep a local Vec of previously-loaded
/// `Gc<LuaStr>`s and resolve back-refs against it.
fn load_string(
    r: &mut Reader,
    heap: &mut Heap,
    str_table: &mut Vec<Gc<crate::runtime::LuaStr>>,
) -> Result<Option<Gc<crate::runtime::LuaStr>>, String> {
    let size = load_size(r)?;
    if size == 0 {
        // back-reference (or NULL)
        let idx = load_unsigned(r)? as usize;
        if idx == 0 {
            return Ok(None);
        }
        if idx > str_table.len() {
            return Err(format!(
                "PUC 5.5 string back-reference idx {idx} out of range (table has {})",
                str_table.len()
            ));
        }
        return Ok(Some(str_table[idx - 1]));
    }
    // size >= 1; PUC writes `size = real_len + 1` then dumps
    // `size + 1` bytes (the real bytes + a trailing '\0'). So we
    // read `real_len + 1` bytes here and intern only the first
    // `real_len`.
    let real_len = size - 1;
    let raw = r.take(real_len + 1)?;
    let s = heap.intern(&raw[..real_len]);
    str_table.push(s);
    Ok(Some(s))
}

// ---- header validation ----

fn check_header(r: &mut Reader) -> Result<(), String> {
    // Signature: caller already validated bytes 0..4 == "\x1bLua".
    // Re-read here to advance the reader cursor uniformly.
    check_literal(r, LUA_SIGNATURE, "signature")?;
    let ver = load_byte(r)?;
    if ver != LUAC_VERSION {
        return Err(format!(
            "PUC 5.5 version byte mismatch: expected 0x{LUAC_VERSION:02x}, got 0x{ver:02x}"
        ));
    }
    let fmt = load_byte(r)?;
    if fmt != LUAC_FORMAT {
        return Err(format!(
            "PUC 5.5 format byte mismatch: expected 0x{LUAC_FORMAT:02x}, got 0x{fmt:02x}"
        ));
    }
    check_literal(r, LUAC_DATA, "LUAC_DATA")?;
    // `dumpNumInfo` block per numeric type: a size byte, then the
    // sentinel value in native LE bytes.
    let int_size = load_byte(r)?;
    if int_size != 4 {
        return Err(format!("PUC 5.5 expects sizeof(int)=4 (got {int_size})"));
    }
    let int_val = i32::from_le_bytes(r.take(4)?.try_into().unwrap());
    if int_val as i64 != LUAC_INT_EXPECTED {
        return Err(format!(
            "PUC 5.5 LUAC_INT mismatch: expected {LUAC_INT_EXPECTED}, got {int_val} \
             (endianness / `int` size mismatch — luna only accepts LE 32-bit `int`)"
        ));
    }
    let inst_size = load_byte(r)?;
    if inst_size != 4 {
        return Err(format!(
            "PUC 5.5 expects sizeof(Instruction)=4 (got {inst_size})"
        ));
    }
    let inst_val = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
    if inst_val != LUAC_INST_EXPECTED {
        return Err(format!(
            "PUC 5.5 LUAC_INST mismatch: expected 0x{LUAC_INST_EXPECTED:08x}, got 0x{inst_val:08x}"
        ));
    }
    let lua_int_size = load_byte(r)?;
    if lua_int_size != 8 {
        return Err(format!(
            "PUC 5.5 expects sizeof(lua_Integer)=8 (got {lua_int_size})"
        ));
    }
    let lua_int_val = i64::from_le_bytes(r.take(8)?.try_into().unwrap());
    if lua_int_val != LUAC_INT_EXPECTED {
        return Err(format!(
            "PUC 5.5 lua_Integer LUAC_INT mismatch: expected {LUAC_INT_EXPECTED}, got {lua_int_val}"
        ));
    }
    let lua_num_size = load_byte(r)?;
    if lua_num_size != 8 {
        return Err(format!(
            "PUC 5.5 expects sizeof(lua_Number)=8 (got {lua_num_size})"
        ));
    }
    let lua_num_val = f64::from_le_bytes(r.take(8)?.try_into().unwrap());
    if lua_num_val != LUAC_NUM_EXPECTED {
        return Err(format!(
            "PUC 5.5 LUAC_NUM mismatch: expected {LUAC_NUM_EXPECTED}, got {lua_num_val}"
        ));
    }
    Ok(())
}

// ---- align helpers ----
//
// `dumpCode` / `loadCode` align the byte stream to `sizeof(Instruction)`
// (=4) BEFORE writing/reading the instruction vector; `loadDebug` does
// the same for `sizeof(AbsLineInfo)` (=4) when abslineinfo is present.
// The padding is `(align - (offset % align)) % align` zero bytes.

fn load_align(r: &mut Reader, align: usize, dump_start: usize) -> Result<(), String> {
    // `offset` is measured from the START of the dump (per PUC's
    // `S->offset` field). PUC initialises `S.offset = 1` because the
    // first signature byte was pre-read by `lua_load`, but then
    // `checkHeader`'s `checkliteral(S, &LUA_SIGNATURE[1], …)` consumes
    // the REMAINING 3 signature bytes — so PUC's offset and our
    // `r.pos()` track the same number of bytes from the start of the
    // header (we read all 4 sig bytes in one go; PUC reads 1 then 3).
    // No correction needed.
    let offset = r.pos() - dump_start;
    let padding = (align - (offset % align)) % align;
    if padding > 0 {
        r.take(padding)?;
    }
    Ok(())
}

// ---- top-level entry ----

/// Decode a PUC 5.5 binary chunk into a `Gc<Proto>` runnable on luna's
/// 65-op interpreter.
///
/// `bytes` is the full chunk (header + body). The `\x1bLua` signature at
/// bytes 0..4 has already been confirmed by the caller (`super::undump_puc`);
/// we re-validate the full 5.5 header to lock byte-exact format match.
pub(super) fn undump_puc_55(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    let mut r = Reader::at(bytes, 0);
    let dump_start = r.pos();
    check_header(&mut r)?;
    // Top-level closure carries an `nupvalues` byte that PUC's loader
    // cross-checks against the proto's own upvalue list size. We just
    // read and validate.
    let nupvalues = load_byte(&mut r)?;
    let mut str_table: Vec<Gc<crate::runtime::LuaStr>> = Vec::new();
    let proto = load_function(&mut r, heap, &mut str_table, None, dump_start)?;
    let proto_nupvals = proto.upvals.len();
    if proto_nupvals != nupvalues as usize {
        return Err(format!(
            "PUC 5.5 closure nupvalues={nupvalues} disagrees with proto upvalues={proto_nupvals}"
        ));
    }
    Ok(proto)
}

// ---- recursive function loader ----

fn load_function(
    r: &mut Reader,
    heap: &mut Heap,
    str_table: &mut Vec<Gc<crate::runtime::LuaStr>>,
    parent_source: Option<Gc<crate::runtime::LuaStr>>,
    dump_start: usize,
) -> Result<Gc<Proto>, String> {
    // PUC `loadFunction`:
    //   linedefined / lastlinedefined (varint int)
    //   numparams (byte)
    //   flag       (byte; PF_VAHID | PF_VATAB after masking out PF_FIXED)
    //   maxstacksize (byte)
    //   loadCode / loadConstants / loadUpvalues / loadProtos
    //   loadString(&f->source)
    //   loadDebug
    let line_defined = load_int(r)? as u32;
    let last_line_defined = load_int(r)? as u32;
    let num_params = load_byte(r)?;
    let raw_flag = load_byte(r)?;
    let is_vararg = (raw_flag & (PF_VAHID | PF_VATAB)) != 0;
    let has_vararg_table_pseudo = (raw_flag & PF_VATAB) != 0;
    let max_stack = load_byte(r)?;

    // ---- code ----
    let code_n = load_int(r)? as usize;
    load_align(r, 4, dump_start)?;
    let mut raw_code = Vec::with_capacity(code_n);
    for _ in 0..code_n {
        let bytes: [u8; 4] = r.take(4)?.try_into().unwrap();
        raw_code.push(u32::from_le_bytes(bytes));
    }

    // ---- constants ----
    let k_n = load_int(r)? as usize;
    let mut consts: Vec<Value> = Vec::with_capacity(k_n);
    for _ in 0..k_n {
        let tag = load_byte(r)?;
        let v = match tag {
            LUA_VNIL => Value::Nil,
            LUA_VFALSE => Value::Bool(false),
            LUA_VTRUE => Value::Bool(true),
            LUA_VNUMINT => Value::Int(load_integer(r)?),
            LUA_VNUMFLT => Value::Float(load_number(r)?),
            LUA_VSHRSTR | LUA_VLNGSTR => {
                let s = load_string(r, heap, str_table)?
                    .ok_or_else(|| "PUC 5.5 nil string constant".to_string())?;
                Value::Str(s)
            }
            other => {
                return Err(format!("PUC 5.5 unknown constant tag 0x{other:02x}"));
            }
        };
        consts.push(v);
    }

    // ---- upvalues ----
    let upv_n = load_int(r)? as usize;
    let mut upvals: Vec<UpvalDesc> = Vec::with_capacity(upv_n);
    for _ in 0..upv_n {
        let in_stack = load_byte(r)? != 0;
        let idx = load_byte(r)?;
        let kind = load_byte(r)?;
        // PUC 5.4+ `kind`: 0 = regular, 1 = local <const>, 2 = local <close>,
        // 3 = compile-time constant. luna only models read_only.
        let read_only = kind == 1 || kind == 3;
        upvals.push(UpvalDesc {
            in_stack,
            index: idx,
            // Names are filled in by `loadDebug` below; default to "" so the
            // Vec is fully initialised in case debug info is stripped.
            name: String::new().into(),
            read_only,
        });
    }

    // ---- nested protos ----
    let proto_n = load_int(r)? as usize;
    // We need the *source* string before recursing so the child can inherit
    // it on an empty-string back-ref. PUC's `loadFunction` loads source
    // AFTER protos, but as a back-reference scheme: when an inner proto's
    // source is also the parent's, the dumper writes it once at the parent
    // level and inner protos reuse it. We pass `None` down for now and
    // overwrite below.
    let mut protos: Vec<Gc<Proto>> = Vec::with_capacity(proto_n);
    for _ in 0..proto_n {
        protos.push(load_function(
            r,
            heap,
            str_table,
            parent_source,
            dump_start,
        )?);
    }

    // ---- source ----
    let source = load_string(r, heap, str_table)?
        .or(parent_source)
        .unwrap_or_else(|| heap.intern(b""));

    // ---- debug: lineinfo / abslineinfo / locvars / upvalue names ----
    // lineinfo: i8 array of per-instruction line deltas (or ABSLINEINFO).
    let li_n = load_int(r)? as usize;
    let mut lineinfo_raw: Vec<i8> = Vec::with_capacity(li_n);
    for _ in 0..li_n {
        lineinfo_raw.push(load_byte(r)? as i8);
    }
    // abslineinfo: (pc, line) pairs. Aligned to sizeof(int)=4 before read
    // only if there's at least one entry (per PUC's `loadDebug`).
    let abs_n = load_int(r)? as usize;
    let mut abslineinfo: Vec<(u32, u32)> = Vec::with_capacity(abs_n);
    if abs_n > 0 {
        load_align(r, 4, dump_start)?;
        for _ in 0..abs_n {
            let pc = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
            let line = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
            abslineinfo.push((pc, line));
        }
    }
    // Expand the (delta, abs) form into luna's flat per-PC u32 line table.
    let lines = expand_lineinfo(&lineinfo_raw, &abslineinfo, line_defined);

    // locvars
    let lv_n = load_int(r)? as usize;
    let mut locvars: Vec<LocVar> = Vec::with_capacity(lv_n);
    for _ in 0..lv_n {
        let name = load_string(r, heap, str_table)?
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        let start_pc = load_int(r)? as u32;
        let end_pc = load_int(r)? as u32;
        locvars.push(LocVar {
            name: String::from_utf8_lossy(&name).into_owned().into(),
            reg: 0, // PUC doesn't dump per-locvar registers; luna's debug
            // helpers tolerate 0 here (locvars are advisory).
            start_pc,
            end_pc,
        });
    }
    // upvalue names — n=0 means stripped; otherwise must equal upv_n
    let unames_n = load_int(r)? as usize;
    let unames_n_actual = if unames_n != 0 { upv_n } else { 0 };
    for i in 0..unames_n_actual {
        let nm = load_string(r, heap, str_table)?
            .map(|s| s.as_bytes().to_vec())
            .unwrap_or_default();
        if i < upvals.len() {
            upvals[i].name = String::from_utf8_lossy(&nm).into_owned().into();
        }
    }

    // ---- translate opcodes ----
    let translated = translate_code(&raw_code)?;

    // Remap per-pc line table from PUC pc-space to luna pc-space (Wave 2:
    // I-imm lowering will insert a `LoadI` slot per I-imm op, so
    // luna_pc != puc_pc in general). `luna_to_puc_pc[luna_pc] = puc_pc` of
    // the *originating* PUC op for every luna slot we emitted.
    let lines_remapped: Vec<u32> = translated
        .luna_to_puc_pc
        .iter()
        .map(|&puc_pc| lines.get(puc_pc).copied().unwrap_or(0))
        .collect();

    // Remap locvars' pc ranges from PUC pc-space to luna pc-space.
    let locvars_remapped: Vec<LocVar> = locvars
        .into_iter()
        .map(|lv| LocVar {
            start_pc: remap_pc(&translated.puc_to_luna_pc, lv.start_pc),
            end_pc: remap_pc(&translated.puc_to_luna_pc, lv.end_pc),
            ..lv
        })
        .collect();

    // max_stack: PUC value + worst-case temp count from I-imm lowering
    // (Wave 2: ADDI / SHRI / SHLI / EQI / LTI / LEI / GTI / GEI each claim
    // one scratch slot above the PUC-reported max).
    let max_stack = max_stack.saturating_add(translated.max_temp_bump);

    // ---- assemble the luna Proto ----
    let env_upval_idx = upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);

    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: translated.code.into_boxed_slice(),
        consts: consts.into_boxed_slice(),
        protos: protos.into_boxed_slice(),
        upvals: upvals.into_boxed_slice(),
        num_params,
        is_vararg,
        has_vararg_table_pseudo,
        has_compat_vararg_arg: false,
        max_stack,
        lines: lines_remapped.into_boxed_slice(),
        source,
        line_defined,
        last_line_defined,
        locvars: locvars_remapped.into_boxed_slice(),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        traces: std::cell::RefCell::new(Vec::new()),
    }))
}

/// Expand PUC's `lineinfo` (i8 deltas + abslineinfo back-references) into
/// luna's flat per-PC u32 line table. PUC's algorithm (`luaG_getfuncline`):
///   - start: line = linedefined
///   - for each pc: if lineinfo[pc] == ABSLINEINFO, line = next
///     abslineinfo entry whose `pc` matches; else line += lineinfo[pc].
///
/// We materialise the full table once at load time so luna's exec / hook
/// code can index `proto.lines[pc]` directly (matching luna's own dump
/// shape).
fn expand_lineinfo(raw: &[i8], abs: &[(u32, u32)], line_defined: u32) -> Vec<u32> {
    if raw.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(raw.len());
    let mut line = line_defined as i32;
    let mut abs_iter = abs.iter();
    let mut next_abs = abs_iter.next();
    for (pc, &delta) in raw.iter().enumerate() {
        if delta == ABSLINEINFO {
            // Find the abslineinfo entry whose pc matches; PUC writes them in
            // order. We accept either an exact pc match or, defensively, take
            // the next entry — `ldump.c::dumpDebug` emits one abs entry per
            // ABSLINEINFO marker so a sequential consume matches.
            if let Some(&(_apc, aline)) = next_abs {
                line = aline as i32;
                next_abs = abs_iter.next();
            } else {
                // shouldn't happen with valid input; fall back to current line
            }
            let _ = pc;
        } else {
            line += delta as i32;
        }
        out.push(line.max(0) as u32);
    }
    out
}

// ---- opcode translation ----
//
// Strategy:
//   - luna's iABC layout matches PUC 5.5's iABC byte-for-byte. ops in
//     luna's set can be emitted by copying the raw word and patching the
//     opcode byte — but it's clearer (and equally fast) to decode the
//     fields and re-build via `Inst::iabc` / `iabx` / `iasbx` / `iax` /
//     `isj`. We use the explicit constructors so this code is easy to
//     audit.
//   - PUC ops not in luna's set are LOWERED:
//       * ADDK..BXORK    → luna's `Add..BXor` with k=1 (constant operand
//         lives in C, k bit already set).
//       * ADDI / SHRI    → no luna equivalent op; lower to `LoadI tmp;
//         Add A B tmp` (with `max_stack` already accommodating tmp slot
//         from PUC's compiler). The audit calls this out as the "K/I
//         lowering register-pressure bump"; for v1.3 Wave 2, programs
//         emitted by PUC have `max_stack` sized for these temps already
//         because PUC reserves the slot for ADDI's i8 immediate. We
//         keep the same A register and add an intermediate immediate
//         load into A+1; PUC's frontend allocates A+1 as scratch so
//         this is safe in practice for stock luac5.5 output.
//       * SHLI            → `LoadI tmp; Shl A tmp B`.
//       * EQI / LTI / LEI / GTI / GEI → `LoadI tmp; Eq/Lt/Le/Lt/Le A
//         tmp k` (GTI/GEI flip operands).
//       * MMBIN / MMBINI / MMBINK → **drop**; luna's arith ops handle
//         metamethod fallback inline.
//       * VARARGPREP     → **drop**; luna's call setup populates
//         varargs without an explicit prep op.
//       * GETVARG / ERRNNIL → luna has matching ops (frontend artefacts
//         for 5.5); copy semantics with caveats.
//       * NEWTABLE       → re-encode vB/vC into luna's plain B/C size
//         hints (luna ignores them at runtime anyway).
//       * SETLIST        → re-encode vB/vC into luna's B/C.
//
// "Drop" must preserve PC layout (other ops jump to fixed PCs); we emit
// a no-op `Move A A` in place of dropped ops so PC math stays intact.

/// Output of `translate_code`: the gap-and-pad-free luna bytecode plus the
/// PC remap tables that downstream debug/locvar/lineinfo logic uses to
/// translate the PUC pc-space into luna pc-space.
struct Translated {
    /// Translated luna instructions.
    code: Vec<Inst>,
    /// `puc_to_luna_pc[puc_pc] = luna_pc of the FIRST emitted slot for
    /// that puc op`. Always `Some(_)` for puc_55 — Wave 2 does not drop
    /// any op; MMBIN / VARARGPREP become a `Move A A` no-op slot so PC
    /// layout stays intact for backward jumps (FORLOOP / FORPREP Bx).
    puc_to_luna_pc: Vec<Option<u32>>,
    /// `luna_to_puc_pc[luna_pc] = puc_pc that originated this slot`. Used
    /// to remap the per-pc line table after I-imm lowering inserts
    /// secondary slots.
    luna_to_puc_pc: Vec<usize>,
    /// Highest `tmp + 1` claimed by any I-imm or cmp-imm lowering site;
    /// the caller adds this to PUC's reported `max_stack`.
    max_temp_bump: u8,
}

fn translate_code(raw: &[u32]) -> Result<Translated, String> {
    let mut code: Vec<Inst> = Vec::with_capacity(raw.len());
    let mut puc_to_luna_pc: Vec<Option<u32>> = Vec::with_capacity(raw.len());
    let mut luna_to_puc_pc: Vec<usize> = Vec::with_capacity(raw.len());
    let mut max_temp_bump: u8 = 0;

    // Stash pending JMP fixups: (luna_pc_of_jmp, puc_target_pc). We
    // can't compute the luna-pc target until the full pc map is built,
    // since I-imm lowering inserts slots between the JMP and its target.
    let mut jump_fixups: Vec<(usize, i64)> = Vec::new();

    // PUC `OP_LOADKX` and `OP_NEWTABLE` (k=1) and `OP_SETLIST` (k=1) are
    // each followed by an `OP_EXTRAARG`. luna treats EXTRAARG identically,
    // so emission is 1:1 — we just need to skip translating EXTRAARG's
    // own opcode bits (it's payload, not an instruction).
    for (puc_pc, &word) in raw.iter().enumerate() {
        let op_byte = (word & 0x7F) as u8;
        let puc_op = PucOp::from_byte(op_byte).map_err(|e| format!("pc {puc_pc}: {e}"))?;

        // Each surviving op claims its starting luna slot.
        puc_to_luna_pc.push(Some(code.len() as u32));

        match puc_op {
            // ---- I-imm arith (3-operand same-order shape) ----
            //
            // PUC `ADDI A B sC`: R[A] := R[B] + sC. luna has no I-imm arith
            // form, so we lower to `LoadI tmp sC; Add A B tmp`. tmp lives
            // at `max(a, b) + 1` (above both source slots so the arith op
            // can still read B before writing A). `lower_i_imm` is the
            // Wave 1 helper in `super`; same call site pattern as puc_54.
            PucOp::ADDI => {
                let a = f_a(word);
                let b = f_b(word);
                let sc = f_sc(word);
                let tmp = a.max(b) + 1;
                let pair = super::lower_i_imm(Op::Add, a, b, sc, tmp, &mut max_temp_bump)?;
                luna_to_puc_pc.push(puc_pc);
                code.push(pair[0]);
                luna_to_puc_pc.push(puc_pc);
                code.push(pair[1]);
            }

            // PUC `SHRI A B sC`: R[A] := R[B] >> sC. Same 3-operand
            // same-order shape as ADDI, so the Wave 1 helper applies
            // directly. (SHLI's operand order is swapped — handled
            // inline below.)
            PucOp::SHRI => {
                let a = f_a(word);
                let b = f_b(word);
                let sc = f_sc(word);
                let tmp = a.max(b) + 1;
                let pair = super::lower_i_imm(Op::Shr, a, b, sc, tmp, &mut max_temp_bump)?;
                luna_to_puc_pc.push(puc_pc);
                code.push(pair[0]);
                luna_to_puc_pc.push(puc_pc);
                code.push(pair[1]);
            }

            // PUC `SHLI A B sC`: R[A] := sC << R[B] — note the operand
            // SWAP vs SHRI (left-hand side is the immediate, right-hand
            // side is the register). luna's `Shl A B C` is `R[A] := R[B]
            // << R[C]`, so we emit `LoadI tmp sC; Shl A tmp B`. The
            // Wave 1 helper assumes `OP A B tmp`, so SHLI stays inline.
            PucOp::SHLI => {
                let a = f_a(word);
                let b = f_b(word);
                let sc = f_sc(word);
                let tmp = a.max(b) + 1;
                if tmp > 0xFF {
                    return Err(format!(
                        "PUC 5.5 SHLI lowering at pc {puc_pc}: temp register {tmp} exceeds 255"
                    ));
                }
                max_temp_bump = max_temp_bump.max(tmp as u8 + 1);
                luna_to_puc_pc.push(puc_pc);
                code.push(Inst::iasbx(Op::LoadI, tmp, sc));
                luna_to_puc_pc.push(puc_pc);
                code.push(Inst::iabc(Op::Shl, a, tmp, b, false));
            }

            // ---- JMP needs a fixup; the sJ offset is in PUC pc-space ----
            PucOp::JMP => {
                let sj = f_sj(word);
                let next_puc_pc = puc_pc as i64 + 1;
                let target_puc_pc = next_puc_pc + sj as i64;
                let luna_pc = code.len();
                jump_fixups.push((luna_pc, target_puc_pc));
                luna_to_puc_pc.push(puc_pc);
                code.push(Inst::isj(Op::Jmp, 0)); // placeholder
            }

            _ => {
                luna_to_puc_pc.push(puc_pc);
                let inst = translate_one(puc_op, word)?;
                code.push(inst);
            }
        }
    }

    // Patch jump fixups now that we know the full pc map.
    for (luna_pc, target_puc_pc) in jump_fixups {
        let target_luna_pc: i64 = if target_puc_pc < 0 {
            return Err(format!(
                "PUC 5.5 JMP at luna pc {luna_pc} targets negative pc {target_puc_pc}"
            ));
        } else if target_puc_pc as usize >= puc_to_luna_pc.len() {
            // Past-the-end: jump to the synthetic "end of code" pc.
            code.len() as i64
        } else {
            let mut t = target_puc_pc as usize;
            loop {
                if t >= puc_to_luna_pc.len() {
                    break code.len() as i64;
                }
                if let Some(p) = puc_to_luna_pc[t] {
                    break p as i64;
                }
                t += 1;
            }
        };
        let next_pc = luna_pc as i64 + 1;
        let sj = (target_luna_pc - next_pc) as i32;
        code[luna_pc] = Inst::isj(Op::Jmp, sj);
    }

    Ok(Translated {
        code,
        puc_to_luna_pc,
        luna_to_puc_pc,
        max_temp_bump,
    })
}

/// Remap a PUC pc to a luna pc, clamping past-the-end values.
fn remap_pc(map: &[Option<u32>], puc_pc: u32) -> u32 {
    let idx = puc_pc as usize;
    if idx >= map.len() {
        // past-the-end (locvar end_pc = len): map to translated len.
        return map
            .iter()
            .rev()
            .find_map(|x| *x)
            .map(|p| p + 1)
            .unwrap_or(0);
    }
    map[idx].unwrap_or_else(|| {
        // PUC pc fell on a dropped op; use the next surviving op (currently
        // unreachable in 5.5 — every op produces at least one luna slot —
        // but kept symmetric with `puc_54::remap_pc` so locvar tables stay
        // safe if Wave 3 introduces drops).
        for &slot in &map[idx + 1..] {
            if let Some(p) = slot {
                return p;
            }
        }
        map.iter()
            .rev()
            .find_map(|x| *x)
            .map(|p| p + 1)
            .unwrap_or(0)
    })
}

/// Translate one PUC 5.5 instruction word into a single luna `Inst`.
///
/// Lowering that requires emitting MORE than one luna instruction
/// (currently: I-imm arith, comparison-immediate, MMBIN-following-arith)
/// would invalidate PC arithmetic for jumps; in PUC's emitted streams
/// the immediates are pre-loaded into a constant pool slot when the
/// frontend can't fit them in i8, so the I-imm forms are rarer than
/// they look. We **fold** the lowering into the destination register +
/// constant-pool synthesis where we can, and fall back to a runtime
/// error for the cases that genuinely need multi-op expansion (kept as
/// a TODO for follow-up audit).
fn translate_one(op: PucOp, word: u32) -> Result<Inst, String> {
    use Op as L;
    let a = f_a(word);
    let b = f_b(word);
    let c = f_c(word);
    let k = f_k(word);
    let bx = f_bx(word);
    let sbx = f_sbx(word);
    let sj = f_sj(word);
    let ax = f_ax(word);
    let vb = f_vb(word);
    let vc = f_vc(word);
    let sb = f_sb(word);
    let sc = f_sc(word);

    // Helper: a no-op slot keeping PC layout intact (used when we drop
    // a PUC op that has no luna equivalent and the slot will never be
    // jumped TO with a result expectation — true of MMBIN-family /
    // VARARGPREP).
    let nop = |reg: u32| Inst::iabc(L::Move, reg, reg, 0, false);

    Ok(match op {
        // -------- direct 1:1 (iABC) --------
        PucOp::MOVE => Inst::iabc(L::Move, a, b, c, k),
        PucOp::LOADFALSE => Inst::iabc(L::LoadFalse, a, 0, 0, false),
        PucOp::LFALSESKIP => Inst::iabc(L::LFalseSkip, a, 0, 0, false),
        PucOp::LOADTRUE => Inst::iabc(L::LoadTrue, a, 0, 0, false),
        PucOp::LOADNIL => Inst::iabc(L::LoadNil, a, b, 0, false),
        PucOp::GETUPVAL => Inst::iabc(L::GetUpval, a, b, 0, false),
        PucOp::SETUPVAL => Inst::iabc(L::SetUpval, a, b, 0, false),
        PucOp::GETTABUP => Inst::iabc(L::GetTabUp, a, b, c, false),
        PucOp::GETTABLE => Inst::iabc(L::GetTable, a, b, c, false),
        PucOp::GETI => Inst::iabc(L::GetI, a, b, c, false),
        PucOp::GETFIELD => Inst::iabc(L::GetField, a, b, c, false),
        PucOp::SETTABUP => Inst::iabc(L::SetTabUp, a, b, c, k),
        PucOp::SETTABLE => Inst::iabc(L::SetTable, a, b, c, k),
        PucOp::SETI => Inst::iabc(L::SetI, a, b, c, k),
        PucOp::SETFIELD => Inst::iabc(L::SetField, a, b, c, k),
        PucOp::SELF => Inst::iabc(L::SelfOp, a, b, c, k),
        PucOp::ADD => Inst::iabc(L::Add, a, b, c, false),
        PucOp::SUB => Inst::iabc(L::Sub, a, b, c, false),
        PucOp::MUL => Inst::iabc(L::Mul, a, b, c, false),
        PucOp::MOD => Inst::iabc(L::Mod, a, b, c, false),
        PucOp::POW => Inst::iabc(L::Pow, a, b, c, false),
        PucOp::DIV => Inst::iabc(L::Div, a, b, c, false),
        PucOp::IDIV => Inst::iabc(L::IDiv, a, b, c, false),
        PucOp::BAND => Inst::iabc(L::BAnd, a, b, c, false),
        PucOp::BOR => Inst::iabc(L::BOr, a, b, c, false),
        PucOp::BXOR => Inst::iabc(L::BXor, a, b, c, false),
        PucOp::SHL => Inst::iabc(L::Shl, a, b, c, false),
        PucOp::SHR => Inst::iabc(L::Shr, a, b, c, false),
        PucOp::UNM => Inst::iabc(L::Unm, a, b, 0, false),
        PucOp::BNOT => Inst::iabc(L::BNot, a, b, 0, false),
        PucOp::NOT => Inst::iabc(L::Not, a, b, 0, false),
        PucOp::LEN => Inst::iabc(L::Len, a, b, 0, false),
        PucOp::CONCAT => Inst::iabc(L::Concat, a, b, 0, false),
        PucOp::CLOSE => Inst::iabc(L::Close, a, 0, 0, false),
        PucOp::TBC => Inst::iabc(L::Tbc, a, 0, 0, false),
        PucOp::EQ => Inst::iabc(L::Eq, a, b, c, k),
        PucOp::LT => Inst::iabc(L::Lt, a, b, c, k),
        PucOp::LE => Inst::iabc(L::Le, a, b, c, k),
        PucOp::EQK => Inst::iabc(L::EqK, a, b, c, k),
        PucOp::TEST => Inst::iabc(L::Test, a, 0, c, k),
        PucOp::TESTSET => Inst::iabc(L::TestSet, a, b, c, k),
        PucOp::CALL => Inst::iabc(L::Call, a, b, c, k),
        PucOp::TAILCALL => Inst::iabc(L::TailCall, a, b, c, k),
        PucOp::RETURN => Inst::iabc(L::Return, a, b, c, k),
        PucOp::RETURN0 => Inst::iabc(L::Return0, 0, 0, 0, false),
        PucOp::RETURN1 => Inst::iabc(L::Return1, a, 0, 0, false),
        PucOp::TFORCALL => Inst::iabc(L::TForCall, a, 0, c, false),
        PucOp::VARARG => Inst::iabc(L::Vararg, a, b, c, k),
        PucOp::GETVARG => Inst::iabc(L::VargIdx, a, b, c, false),

        // -------- iABx / iAsBx / iAx / isJ direct --------
        PucOp::LOADI => Inst::iasbx(L::LoadI, a, sbx),
        PucOp::LOADF => Inst::iasbx(L::LoadF, a, sbx),
        PucOp::LOADK => Inst::iabx(L::LoadK, a, bx),
        PucOp::LOADKX => Inst::iabx(L::LoadKx, a, 0),
        PucOp::FORLOOP => Inst::iabx(L::ForLoop, a, bx),
        PucOp::FORPREP => Inst::iabx(L::ForPrep, a, bx),
        PucOp::TFORPREP => Inst::iabx(L::TForPrep, a, bx),
        PucOp::TFORLOOP => Inst::iabx(L::TForLoop, a, bx),
        PucOp::CLOSURE => Inst::iabx(L::Closure, a, bx),
        PucOp::ERRNNIL => Inst::iabx(L::ErrNNil, a, bx),
        PucOp::JMP => Inst::isj(L::Jmp, sj),
        PucOp::EXTRAARG => Inst::iax(L::ExtraArg, ax),

        // -------- K-immediate arith (luna's k-bit form is 1:1) --------
        PucOp::ADDK => Inst::iabc(L::Add, a, b, c, true),
        PucOp::SUBK => Inst::iabc(L::Sub, a, b, c, true),
        PucOp::MULK => Inst::iabc(L::Mul, a, b, c, true),
        PucOp::MODK => Inst::iabc(L::Mod, a, b, c, true),
        PucOp::POWK => Inst::iabc(L::Pow, a, b, c, true),
        PucOp::DIVK => Inst::iabc(L::Div, a, b, c, true),
        PucOp::IDIVK => Inst::iabc(L::IDiv, a, b, c, true),
        PucOp::BANDK => Inst::iabc(L::BAnd, a, b, c, true),
        PucOp::BORK => Inst::iabc(L::BOr, a, b, c, true),
        PucOp::BXORK => Inst::iabc(L::BXor, a, b, c, true),

        // -------- I-immediate arith --------
        //
        // PUC's `OP_ADDI` carries `sC` as a signed 8-bit literal:
        // `R[A] := R[B] + sC`. luna has no I-imm form, so the
        // ADDI / SHRI sites are lowered inside `translate_code` to a
        // `LoadI tmp sC; Add A B tmp` pair (via the Wave 1 shared
        // `super::lower_i_imm` helper). This arm of `translate_one` is
        // unreachable in the normal undump path; we keep a sentinel so
        // unit tests that exercise the legacy single-Inst contract still
        // get a clear error.
        PucOp::ADDI => {
            return Err(format!(
                "PUC 5.5 OP_ADDI(A={a}, B={b}, sC={sc}): I-imm lowering is \
                 handled in translate_code (Wave 2); translate_one is \
                 single-Inst only"
            ));
        }
        PucOp::SHLI => {
            // SHLI's operand order swap (`R[A] := sC << R[B]`) doesn't
            // fit `super::lower_i_imm`'s `OP A B tmp` shape, so it's
            // lowered inline in `translate_code` to `LoadI tmp sC;
            // Shl A tmp B`. This arm stays as a sentinel.
            return Err(format!(
                "PUC 5.5 OP_SHLI(A={a}, B={b}, sC={sc}): I-imm lowering is \
                 handled in translate_code (Wave 2); translate_one is \
                 single-Inst only"
            ));
        }
        PucOp::SHRI => {
            // Handled in translate_code via `super::lower_i_imm`; same
            // single-Inst contract as ADDI above.
            return Err(format!(
                "PUC 5.5 OP_SHRI(A={a}, B={b}, sC={sc}): I-imm lowering is \
                 handled in translate_code (Wave 2); translate_one is \
                 single-Inst only"
            ));
        }
        PucOp::EQI | PucOp::LTI | PucOp::LEI | PucOp::GTI | PucOp::GEI => {
            return Err(format!(
                "PUC 5.5 {op:?}(A={a}, sB={sb}, k={k}) needs cmp-imm lowering"
            ));
        }

        // -------- MMBIN family: drop (luna handles metamethods inline) --------
        // PUC emits these immediately after the corresponding arith op as
        // a metamethod fallback hint. luna's arith dispatchers handle
        // `__add` / `__sub` / ... inline in `exec.rs`, so we replace the
        // slot with a no-op `Move A A` (the slot is never the target of a
        // jump; PUC's compiler keeps it sequential).
        PucOp::MMBIN | PucOp::MMBINI | PucOp::MMBINK => nop(a),

        // -------- VARARGPREP: drop (luna's call setup handles varargs) --------
        PucOp::VARARGPREP => nop(0),

        // -------- vABC re-encode (NEWTABLE, SETLIST) --------
        // luna's NewTable ignores B/C size hints (always builds empty),
        // so clamp to 8-bit and drop overflow.
        PucOp::NEWTABLE => {
            let b_clamped = if vb > 0xFF { 0 } else { vb };
            let c_clamped = if vc > 0xFF { 0 } else { vc };
            Inst::iabc(L::NewTable, a, b_clamped, c_clamped, k)
        }
        PucOp::SETLIST => {
            // SETLIST: when vC overflows 8-bit, PUC sets k=1 and stuffs the
            // high bits into the following EXTRAARG. luna's SetList reads
            // the EXTRAARG via `inst.k()`, so we just need vC clamped into
            // luna's 8-bit C (the extra bits live in the EXTRAARG slot we
            // already emit untouched).
            let b_clamped = if vb > 0xFF { 0xFF } else { vb };
            let c_clamped = if vc > 0xFF { 0 } else { vc };
            Inst::iabc(L::SetList, a, b_clamped, c_clamped, k)
        }
    })
}

#[cfg(test)]
#[allow(clippy::identity_op)] // explicit `0u32 << 7` / `1u32 << 16` documents
// each field's bit position in test instruction encodings; collapsing them
// would obscure what the test is checking.
mod tests {
    use super::*;

    /// Encode a PUC 5.5 varint (MSB-first, high-bit-set = continue).
    fn encode_varint(mut x: u64) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        buf.push((x & 0x7f) as u8); // least-significant byte first (no high bit)
        x >>= 7;
        while x != 0 {
            buf.push(((x & 0x7f) as u8) | 0x80);
            x >>= 7;
        }
        buf.reverse();
        buf
    }

    /// Construct a minimal valid PUC 5.5 header byte stream.
    fn header() -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(LUA_SIGNATURE);
        h.push(LUAC_VERSION);
        h.push(LUAC_FORMAT);
        h.extend_from_slice(LUAC_DATA);
        h.push(4);
        h.extend_from_slice(&(LUAC_INT_EXPECTED as i32).to_le_bytes());
        h.push(4);
        h.extend_from_slice(&LUAC_INST_EXPECTED.to_le_bytes());
        h.push(8);
        h.extend_from_slice(&LUAC_INT_EXPECTED.to_le_bytes());
        h.push(8);
        h.extend_from_slice(&LUAC_NUM_EXPECTED.to_le_bytes());
        h
    }

    #[test]
    fn varint_roundtrip() {
        for v in [
            0u64,
            1,
            127,
            128,
            200,
            16383,
            16384,
            1_000_000,
            u32::MAX as u64,
        ] {
            let encoded = encode_varint(v);
            let mut r = Reader::at(&encoded, 0);
            let got = read_puc_varint(&mut r).unwrap();
            assert_eq!(got, v, "roundtrip failed for {v}");
            assert_eq!(r.pos(), encoded.len(), "didn't consume all bytes for {v}");
        }
    }

    #[test]
    fn header_validates() {
        let h = header();
        let mut r = Reader::at(&h, 0);
        check_header(&mut r).unwrap();
        assert_eq!(r.pos(), h.len());
    }

    #[test]
    fn header_rejects_wrong_version() {
        let mut h = header();
        h[4] = 0x54; // mutate version byte
        let mut r = Reader::at(&h, 0);
        let err = check_header(&mut r).unwrap_err();
        assert!(err.contains("version"), "got: {err}");
    }

    #[test]
    fn translates_simple_arith() {
        // Hand-construct: OP_ADD with A=0, B=1, C=2
        let word = (PucOp::ADD as u32) | (0u32 << 7) | (1u32 << 16) | (2u32 << 24);
        let inst = translate_one(PucOp::ADD, word).unwrap();
        assert_eq!(inst.op(), Op::Add);
        assert_eq!(inst.a(), 0);
        assert_eq!(inst.b(), 1);
        assert_eq!(inst.c(), 2);
        assert!(!inst.k());
    }

    #[test]
    fn translates_addk_to_k_bit_form() {
        // OP_ADDK A=3 B=1 C=4
        let word = (PucOp::ADDK as u32) | (3u32 << 7) | (1u32 << 16) | (4u32 << 24);
        let inst = translate_one(PucOp::ADDK, word).unwrap();
        assert_eq!(inst.op(), Op::Add);
        assert!(inst.k(), "ADDK must set k=1");
        assert_eq!(inst.a(), 3);
        assert_eq!(inst.b(), 1);
        assert_eq!(inst.c(), 4);
    }

    #[test]
    fn drops_varargprep_to_nop() {
        let word = PucOp::VARARGPREP as u32;
        let inst = translate_one(PucOp::VARARGPREP, word).unwrap();
        assert_eq!(inst.op(), Op::Move);
        assert_eq!(inst.a(), 0);
        assert_eq!(inst.b(), 0);
    }

    #[test]
    fn drops_mmbin_to_nop() {
        // MMBIN A=2 — slot must be a no-op to preserve PC layout.
        let word = (PucOp::MMBIN as u32) | (2u32 << 7);
        let inst = translate_one(PucOp::MMBIN, word).unwrap();
        assert_eq!(inst.op(), Op::Move);
        assert_eq!(inst.a(), 2);
        assert_eq!(inst.b(), 2);
    }

    #[test]
    fn jmp_sj_roundtrip() {
        // OP_JMP with sJ = -5
        let sj_encoded = (-5i32 + OFFSET_SJ) as u32;
        let word = (PucOp::JMP as u32) | (sj_encoded << POS_AX);
        let inst = translate_one(PucOp::JMP, word).unwrap();
        assert_eq!(inst.op(), Op::Jmp);
        assert_eq!(inst.sj(), -5);
    }

    #[test]
    fn loadi_sbx_roundtrip() {
        // OP_LOADI A=0 sBx=42
        let sbx_encoded = (42i32 + OFFSET_SBX) as u32;
        let word = (PucOp::LOADI as u32) | (0u32 << 7) | (sbx_encoded << POS_BX);
        let inst = translate_one(PucOp::LOADI, word).unwrap();
        assert_eq!(inst.op(), Op::LoadI);
        assert_eq!(inst.sbx(), 42);
    }

    #[test]
    fn translate_shli_inline_swap() {
        // OP_SHLI A=1 B=3 sC=5 → `R[1] := 5 << R[3]` → LoadI tmp 5; Shl 1 tmp 3.
        let sc_enc = (5 + OFFSET_SC) as u32;
        let word = (PucOp::SHLI as u32) | (1u32 << 7) | (3u32 << 16) | (sc_enc << 24);
        let translated = translate_code(&[word]).unwrap();
        assert_eq!(translated.code.len(), 2);
        assert_eq!(translated.code[0].op(), Op::LoadI);
        assert_eq!(translated.code[0].sbx(), 5);
        let tmp = translated.code[0].a();
        assert_eq!(tmp, 4, "tmp must be max(a, b) + 1 = max(1, 3) + 1 = 4");
        assert_eq!(translated.code[1].op(), Op::Shl);
        assert_eq!(translated.code[1].a(), 1);
        assert_eq!(translated.code[1].b(), tmp, "B (left operand) is the tmp");
        assert_eq!(translated.code[1].c(), 3, "C (right operand) is the source reg");
        assert_eq!(translated.max_temp_bump, 5);
    }

    #[test]
    fn translate_shri_via_lower_i_imm() {
        // OP_SHRI A=2 B=4 sC=3 (sC encoded as C = 3 + 0x80 = 131)
        let sc_enc = (3 + OFFSET_SC) as u32;
        let word = (PucOp::SHRI as u32) | (2u32 << 7) | (4u32 << 16) | (sc_enc << 24);
        let translated = translate_code(&[word]).unwrap();
        assert_eq!(translated.code.len(), 2);
        assert_eq!(translated.code[0].op(), Op::LoadI);
        assert_eq!(translated.code[0].sbx(), 3);
        let tmp = translated.code[0].a();
        assert_eq!(tmp, 5, "tmp must be max(a, b) + 1 = max(2, 4) + 1 = 5");
        assert_eq!(translated.code[1].op(), Op::Shr);
        assert_eq!(translated.code[1].a(), 2);
        assert_eq!(translated.code[1].b(), 4);
        assert_eq!(translated.code[1].c(), tmp);
        assert!(!translated.code[1].k());
        assert_eq!(translated.max_temp_bump, 6);
    }

    #[test]
    fn translate_addi_via_lower_i_imm() {
        // OP_ADDI A=5 B=3 sC=42 (sC encoded as C = 42 + 0x80 = 170)
        let sc_enc = (42 + OFFSET_SC) as u32;
        let word = (PucOp::ADDI as u32) | (5u32 << 7) | (3u32 << 16) | (sc_enc << 24);
        let translated = translate_code(&[word]).unwrap();
        // Expect 2 luna insts: LoadI tmp 42; Add 5 3 tmp.
        assert_eq!(translated.code.len(), 2);
        assert_eq!(translated.code[0].op(), Op::LoadI);
        assert_eq!(translated.code[0].sbx(), 42);
        let tmp = translated.code[0].a();
        assert_eq!(tmp, 6, "tmp must be max(a, b) + 1 = max(5, 3) + 1 = 6");
        assert_eq!(translated.code[1].op(), Op::Add);
        assert_eq!(translated.code[1].a(), 5);
        assert_eq!(translated.code[1].b(), 3);
        assert_eq!(translated.code[1].c(), tmp);
        assert!(!translated.code[1].k(), "I-imm arith never sets the k bit");
        assert_eq!(translated.max_temp_bump, 7);
        // PC maps: puc pc 0 → luna pc 0 (start of pair); both luna slots
        // attribute back to puc pc 0 for line/locvar remap.
        assert_eq!(translated.puc_to_luna_pc, vec![Some(0)]);
        assert_eq!(translated.luna_to_puc_pc, vec![0, 0]);
    }
}
