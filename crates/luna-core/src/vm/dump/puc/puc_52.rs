//! PUC Lua 5.2 binary chunk → luna `Proto` translator (Phase LB Wave 2).
//!
//! Ports `lua-5.2.4/src/lundump.c` (LoadHeader / LoadFunction / Load*)
//! byte-for-byte plus an opcode translation pass that re-encodes 5.2's
//! 6-bit-op iABC layout into luna's 7-bit-op layout (`Inst::iabc/iabx/
//! iasbx/iax/isj` constructors). 5.2 is structurally simpler than 5.1:
//! `_ENV` is already a real upvalue (so no `GETGLOBAL` synth), and
//! `TFORCALL`/`TFORLOOP` are already split (so no `TForPrep` injection).
//!
//! References (`scratchpad/lua52/lua-5.2.4/src/`):
//! - `lundump.c` — binary chunk loader
//! - `lopcodes.h` — `OpCode` enum + bit layout (`SIZE_OP=6`, `SIZE_A=8`,
//!   `SIZE_C=9`, `SIZE_B=9`; opcode at bit 0; A at 6; C at 14; B at 23)
//! - `lobject.c` — `luaO_fb2int` for `NEWTABLE` size hints
//!
//! Audit: `.dev/rfcs/v1.3-audit-puc-luac-formats.md` §"Lua 5.2 (~40 ops)".

use super::super::reader::Reader;
use crate::runtime::Value;
use crate::runtime::function::{LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::vm::isa::{self, Inst, Op};

/// PUC 5.2 header bytes, byte-for-byte per `luaU_header`. luna only
/// accepts LE chunks with `sizeof(int)=4`, `sizeof(size_t)=8`,
/// `sizeof(Instruction)=4`, `sizeof(lua_Number)=8`, and the conventional
/// "lua_Number is float not integral" flag (`0`).
const HEADER_52: &[u8] = &[
    0x1b, b'L', b'u', b'a', // signature
    0x52, // VERSION (5*16 + 2)
    0x00, // FORMAT (official)
    0x01, // endianness: little-endian
    0x04, // sizeof(int)
    0x08, // sizeof(size_t)
    0x04, // sizeof(Instruction)
    0x08, // sizeof(lua_Number)
    0x00, // lua_Number is float (not integral)
    // LUAC_TAIL = "\x19\x93\r\n\x1a\n"
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
];

// ─── PUC 5.2 opcode IDs (per lopcodes.h, 0-indexed) ────────────────
const P_MOVE: u8 = 0;
const P_LOADK: u8 = 1;
const P_LOADKX: u8 = 2;
const P_LOADBOOL: u8 = 3;
const P_LOADNIL: u8 = 4;
const P_GETUPVAL: u8 = 5;
const P_GETTABUP: u8 = 6;
const P_GETTABLE: u8 = 7;
const P_SETTABUP: u8 = 8;
const P_SETUPVAL: u8 = 9;
const P_SETTABLE: u8 = 10;
const P_NEWTABLE: u8 = 11;
const P_SELF: u8 = 12;
const P_ADD: u8 = 13;
const P_SUB: u8 = 14;
const P_MUL: u8 = 15;
const P_DIV: u8 = 16;
const P_MOD: u8 = 17;
const P_POW: u8 = 18;
const P_UNM: u8 = 19;
const P_NOT: u8 = 20;
const P_LEN: u8 = 21;
const P_CONCAT: u8 = 22;
const P_JMP: u8 = 23;
const P_EQ: u8 = 24;
const P_LT: u8 = 25;
const P_LE: u8 = 26;
const P_TEST: u8 = 27;
const P_TESTSET: u8 = 28;
const P_CALL: u8 = 29;
const P_TAILCALL: u8 = 30;
const P_RETURN: u8 = 31;
const P_FORLOOP: u8 = 32;
const P_FORPREP: u8 = 33;
const P_TFORCALL: u8 = 34;
const P_TFORLOOP: u8 = 35;
const P_SETLIST: u8 = 36;
const P_CLOSURE: u8 = 37;
const P_VARARG: u8 = 38;
const P_EXTRAARG: u8 = 39;

/// PUC 5.2 iABC field layout: opcode is the low 6 bits, A is the next 8,
/// then C (9), then B (9). Bx is C|B treated as one 18-bit field.
/// RK bit: high bit of the 9-bit slot signals "this is a K index".
const RK_BIT: u32 = 1 << 8; // BITRK = 1 << (SIZE_B - 1) = 1 << 8

#[derive(Clone, Copy, Debug)]
struct PucInst {
    op: u8,
    a: u32,
    b: u32, // 9-bit raw (top bit is K)
    c: u32, // 9-bit raw (top bit is K)
}

impl PucInst {
    fn decode(raw: u32) -> PucInst {
        let op = (raw & 0x3F) as u8;
        let a = (raw >> 6) & 0xFF;
        let c = (raw >> 14) & 0x1FF;
        let b = (raw >> 23) & 0x1FF;
        PucInst { op, a, b, c }
    }
    fn bx(self) -> u32 {
        // B|C as one 18-bit field (B is the high 9 bits)
        (self.b << 9) | self.c
    }
    fn sbx(self) -> i32 {
        // 5.2 sBx bias = MAXARG_sBx = (2^18 - 1) >> 1 = 131071
        self.bx() as i32 - 131071
    }
    fn ax(self) -> u32 {
        // A | C | B as one 26-bit field (used by EXTRAARG)
        (self.b << 17) | (self.c << 8) | self.a
    }
}

/// Decode `R[A][k:string] := R[C]/K[C]`-style RK operand: returns
/// `(index, is_const)`. `index` fits in 8 bits because luna's instruction
/// layout has only 8-bit B/C fields (PUC 5.2 used 9-bit B/C plus the
/// RK-flag bit; the payload is always ≤ 255 in practice — PUC's
/// `luaK_exp2RK` caps constants at `MAXINDEXRK = 255`).
fn decode_rk(field: u32) -> Result<(u8, bool), String> {
    let is_k = (field & RK_BIT) != 0;
    let idx = field & 0xFF;
    // The remaining 9th bit (RK_BIT) is the K flag; bit 8 in the index
    // would only matter if PUC allowed indices >= 256, which it doesn't.
    if (field & !RK_BIT) > 0xFF {
        return Err(format!("PUC 5.2 RK index out of range: {field}"));
    }
    Ok((idx as u8, is_k))
}

/// PUC `luaO_fb2int`: decode the floating-point byte used for
/// `NEWTABLE` size hints. Mantissa in low 3 bits, exponent in next 5.
fn fb2int(x: u32) -> u32 {
    let e = (x >> 3) & 0x1F;
    if e == 0 {
        x
    } else {
        // ((x & 7) + 8) << (e - 1)
        ((x & 7) + 8) << (e - 1)
    }
}

/// Saturating fb-int → u8 for re-encoding NEWTABLE size hints into
/// luna's 8-bit B/C fields. luna treats B/C as plain hints (not fb-int),
/// so we decode then clamp to 0xFF.
fn fb_to_hint_u8(x: u32) -> u32 {
    let n = fb2int(x);
    n.min(0xFF)
}

// ─── chunk-level entry point ────────────────────────────────────────

pub(super) fn undump(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    if bytes.len() < HEADER_52.len() {
        return Err("truncated PUC 5.2 binary chunk (header)".to_string());
    }
    // Validate header byte-for-byte. The endianness byte at 6, sizeof
    // fields, and integral flag must all match — luna only loads LE,
    // 32-bit-int + 64-bit-size_t/Number chunks. PUC produces all of
    // those on essentially every desktop/64-bit build.
    if &bytes[..HEADER_52.len()] != HEADER_52 {
        // Pinpoint the most useful mismatch reason for the test suite
        // and for embedder debugging. The byte positions are stable.
        if &bytes[..4] != b"\x1bLua" {
            return Err("not a PUC binary chunk (bad signature)".to_string());
        }
        if bytes[4] != 0x52 {
            return Err(format!(
                "PUC 5.2 loader: version byte 0x{:02x} != 0x52",
                bytes[4]
            ));
        }
        if bytes[6] != 0x01 {
            return Err("PUC 5.2 loader: only little-endian chunks supported".to_string());
        }
        if bytes[7] != 0x04 {
            return Err(format!(
                "PUC 5.2 loader: expected sizeof(int)=4, got {}",
                bytes[7]
            ));
        }
        if bytes[8] != 0x08 {
            return Err(format!(
                "PUC 5.2 loader: expected sizeof(size_t)=8, got {}",
                bytes[8]
            ));
        }
        if bytes[9] != 0x04 {
            return Err(format!(
                "PUC 5.2 loader: expected sizeof(Instruction)=4, got {}",
                bytes[9]
            ));
        }
        if bytes[10] != 0x08 {
            return Err(format!(
                "PUC 5.2 loader: expected sizeof(lua_Number)=8, got {}",
                bytes[10]
            ));
        }
        if bytes[11] != 0x00 {
            return Err(
                "PUC 5.2 loader: integral lua_Number not supported (expected float)".to_string(),
            );
        }
        return Err("PUC 5.2 loader: header tail mismatch".to_string());
    }
    let mut r = Reader::at(bytes, HEADER_52.len());
    let proto = read_function(&mut r, heap, None)?;
    // PUC's LoadHeader leaves the read position just past the header;
    // there is no top-level trailer in 5.2 (the function dump consumes
    // the rest). Treat trailing bytes as an error like luna's own
    // undumper does.
    if r.pos() != bytes.len() {
        return Err(format!(
            "PUC 5.2 loader: trailing bytes (read {} of {})",
            r.pos(),
            bytes.len()
        ));
    }
    Ok(proto)
}

// ─── per-Proto reader (mirrors LoadFunction) ───────────────────────

fn load_size(r: &mut Reader) -> Result<u64, String> {
    // 5.2 uses sizeof(size_t)=8 here per the header gate above.
    Ok(u64::from_le_bytes(r.take(8)?.try_into().unwrap()))
}

fn load_int(r: &mut Reader) -> Result<i32, String> {
    // 5.2 uses sizeof(int)=4 here per the header gate above. PUC stores
    // signed; negative values are corrupt and PUC errors on them too.
    let v = i32::from_le_bytes(r.take(4)?.try_into().unwrap());
    if v < 0 {
        return Err("PUC 5.2 loader: corrupt negative int".to_string());
    }
    Ok(v)
}

fn load_byte(r: &mut Reader) -> Result<u8, String> {
    r.u8()
}

fn load_number(r: &mut Reader) -> Result<f64, String> {
    Ok(f64::from_bits(u64::from_le_bytes(
        r.take(8)?.try_into().unwrap(),
    )))
}

/// PUC `LoadString`: size_t length prefix, payload includes a trailing
/// `'\0'` that's stripped before the string is interned. A `size == 0`
/// means "no string" (NULL TString) — represented here as None.
fn load_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, String> {
    let n = load_size(r)? as usize;
    if n == 0 {
        return Ok(None);
    }
    let raw = r.take(n)?;
    // PUC pre-allocates `size` bytes and the C source writes `size-1`
    // chars then a trailing `'\0'`. We strip that null.
    if raw.last() != Some(&0) {
        return Err("PUC 5.2 loader: string missing trailing NUL".to_string());
    }
    Ok(Some(&raw[..n - 1]))
}

fn load_constants(r: &mut Reader, heap: &mut Heap) -> Result<Box<[Value]>, String> {
    let n = load_int(r)? as usize;
    let mut consts = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = load_byte(r)?;
        let v = match tag {
            // LUA_TNIL = 0
            0 => Value::Nil,
            // LUA_TBOOLEAN = 1 — payload is one byte (0/1)
            1 => Value::Bool(load_byte(r)? != 0),
            // LUA_TNUMBER = 3 — lua_Number = double (5.2 has no Int subtype)
            3 => Value::Float(load_number(r)?),
            // LUA_TSTRING = 4
            4 => {
                let s = load_string(r)?.unwrap_or(b"");
                Value::Str(heap.intern(s))
            }
            t => return Err(format!("PUC 5.2 loader: bad constant tag {t}")),
        };
        consts.push(v);
    }
    Ok(consts.into_boxed_slice())
}

fn load_upvalues(r: &mut Reader) -> Result<Vec<UpvalDesc>, String> {
    let n = load_int(r)? as usize;
    let mut upvals = Vec::with_capacity(n);
    for _ in 0..n {
        let in_stack = load_byte(r)? != 0;
        let index = load_byte(r)?;
        upvals.push(UpvalDesc {
            in_stack,
            index,
            // PUC writes the upval *names* in the debug section
            // (LoadDebug below); leave a placeholder we'll backfill,
            // or empty if the chunk was stripped.
            name: String::new().into(),
            read_only: false,
        });
    }
    Ok(upvals)
}

fn load_debug(
    r: &mut Reader,
    heap: &mut Heap,
    upvals: &mut [UpvalDesc],
) -> Result<
    (
        Gc<crate::runtime::string::LuaStr>,
        Box<[u32]>,
        Box<[LocVar]>,
    ),
    String,
> {
    // source (may be None if the chunk was stripped; PUC writes a
    // zero-size string in that case)
    let source_bytes = load_string(r)?.unwrap_or(b"");
    let source = heap.intern(source_bytes);

    // lineinfo: int[]
    let n = load_int(r)? as usize;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        // PUC lineinfo is `int` (signed 32-bit). luna stores u32; PUC's
        // line numbers are 1-based positives.
        let raw = i32::from_le_bytes(r.take(4)?.try_into().unwrap());
        lines.push(raw.max(0) as u32);
    }

    // locvars
    let n = load_int(r)? as usize;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = load_string(r)?.unwrap_or(b"");
        let start_pc = load_int(r)? as u32;
        let end_pc = load_int(r)? as u32;
        locvars.push(LocVar {
            name: String::from_utf8_lossy(name).into_owned().into(),
            // PUC LocVar stores no register — but luna's tracks `reg`
            // for getlocal naming. 0 is the safe default; luna's
            // dispatcher tolerates an unmapped locvars table.
            reg: 0,
            start_pc,
            end_pc,
        });
    }

    // upvalue names (one per upval)
    let n = load_int(r)? as usize;
    if n != upvals.len() && n != 0 {
        // PUC tolerates n != sizeupvalues only when n == 0 (stripped);
        // otherwise it's a structural mismatch.
        return Err(format!(
            "PUC 5.2 loader: upvalue-name count {n} != upvalue count {}",
            upvals.len()
        ));
    }
    for i in 0..n {
        let name = load_string(r)?.unwrap_or(b"");
        upvals[i].name = String::from_utf8_lossy(name).into_owned().into();
    }

    Ok((source, lines.into_boxed_slice(), locvars.into_boxed_slice()))
}

fn read_function(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::string::LuaStr>>,
) -> Result<Gc<Proto>, String> {
    let line_defined = load_int(r)? as u32;
    let last_line_defined = load_int(r)? as u32;
    let num_params = load_byte(r)?;
    let is_vararg = load_byte(r)? != 0;
    let max_stack = load_byte(r)?;

    // code: int n, then n * sizeof(Instruction) raw u32s.
    let n = load_int(r)? as usize;
    let mut raw_code = Vec::with_capacity(n);
    for _ in 0..n {
        raw_code.push(u32::from_le_bytes(r.take(4)?.try_into().unwrap()));
    }

    let consts = load_constants(r, heap)?;

    // PUC 5.2 order: LoadFunction → LoadCode → LoadConstants →
    // LoadConstants reads nested protos inline after the constants
    // table (`LoadConstants` ends with the protos-vector). Replicate
    // that here. See lundump.c:96-132.
    let n = load_int(r)? as usize;
    let mut protos = Vec::with_capacity(n);
    for _ in 0..n {
        // child source not known yet — we'll thread `self.source` after
        // LoadDebug runs. PUC's loader has the same issue but solves it
        // by recursing depth-first; here, child Protos load their own
        // source bytes (in LoadDebug they'll be non-empty unless
        // stripped, in which case they inherit the parent's per luna's
        // `r_proto` convention).
        protos.push(read_function(r, heap, parent_source)?);
    }

    let mut upvals = load_upvalues(r)?;
    let (mut source, lines, locvars) = load_debug(r, heap, &mut upvals)?;
    // PUC LoadDebug source==empty means stripped; inherit parent's so
    // tracebacks still point somewhere sane (matches luna's own
    // `r_proto` convention).
    if source.as_bytes().is_empty()
        && let Some(p) = parent_source
    {
        source = p;
    }

    // Translate the raw 5.2 instruction stream into luna ops.
    let (code, translated_lines, max_temp_bump) = translate_code(&raw_code, &lines, &consts)?;
    // K-on-LHS / K-on-both arith, EQ/LT/LE with constant operands, and
    // GETTABUP-with-register-key all lower through scratch temp registers
    // allocated above PUC's reported `max_stack`. Bump the frame so the
    // runtime growth check accounts for them. PUC tops out below 250 so
    // saturating add is safe.
    let max_stack = max_stack.saturating_add(max_temp_bump);

    // PUC `_ENV` lookup: by 5.2 convention the main chunk's first
    // upvalue is named `_ENV`. Compute the cached index for the VM's
    // `Op::Closure` fast path.
    let env_upval_idx = upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);

    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: code.into_boxed_slice(),
        consts,
        protos: protos.into_boxed_slice(),
        upvals: upvals.into_boxed_slice(),
        num_params,
        is_vararg,
        has_vararg_table_pseudo: false,
        // PUC 5.2 dropped LUAI_COMPAT_VARARG; the hidden `arg` local
        // does not exist.
        has_compat_vararg_arg: false,
        max_stack,
        lines: translated_lines.into_boxed_slice(),
        source,
        line_defined,
        last_line_defined,
        locvars,
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
    }))
}

// ─── opcode translation ─────────────────────────────────────────────

/// Translate the raw 5.2 instruction stream into luna's 7-bit-op
/// stream. Two passes:
///
/// 1. **Sizing pass**: per source PC, determine how many luna
///    instructions we'll emit. Most ops translate 1→1; `OP_JMP A sBx`
///    with `A != 0` expands to `Close (A-1); Jmp sBx` (1→2), and
///    `OP_SETLIST A B 0` (C-from-following-word) translates to a
///    `SetList k=true` followed by an `ExtraArg` (1→2). Build a map
///    `src_pc → dst_pc` and remember the size of each source op.
///
/// 2. **Emit pass**: walk source ops and emit the translated form, using
///    the map to re-map jump targets so that PUC's PC-relative offsets
///    (sBx for `OP_JMP` / `OP_FORLOOP` / `OP_FORPREP` / `OP_TFORLOOP`)
///    point at the right post-translation instruction.
///
/// Per-PC line info follows the source-PC layout under PUC; when a
/// source op expands to multiple luna ops, all emitted ops carry the
/// same source line so debug.getinfo lookups produce the PUC-equivalent
/// line numbers.
fn translate_code(
    raw_code: &[u32],
    lines: &[u32],
    _consts: &[Value],
) -> Result<(Vec<Inst>, Vec<u32>, u8), String> {
    let n_src = raw_code.len();
    // Sizing pass: for each src PC, the number of luna ops we'll emit.
    // Also track which src PCs are "data payloads" that follow a
    // SETLIST-C=0 — they must NOT contribute to dst_pc themselves; the
    // raw u32 has already been consumed by the SETLIST translation as
    // the ExtraArg `ax` value.
    let mut src_to_dst = Vec::with_capacity(n_src + 1);
    let mut sizes: Vec<u32> = vec![0; n_src];
    let mut is_data_payload: Vec<bool> = vec![false; n_src];
    let mut dst_pc: u32 = 0;
    let mut src_pc = 0;
    while src_pc < n_src {
        src_to_dst.push(dst_pc);
        let p = PucInst::decode(raw_code[src_pc]);
        let size = src_size(p)?;
        sizes[src_pc] = size;
        dst_pc = dst_pc
            .checked_add(size)
            .ok_or("translated code too large")?;
        if p.op == P_SETLIST && p.c == 0 {
            // Next src PC holds the raw C value, not an opcode. Mark
            // it as a data payload — its src_to_dst entry mirrors the
            // current dst_pc (so jumps targeting it land on the
            // following luna op, which is wrong but PUC never emits
            // jumps targeting the C-payload word).
            src_pc += 1;
            if src_pc >= n_src {
                return Err(
                    "PUC 5.2 translator: SETLIST C=0 at end of code (missing C payload)"
                        .to_string(),
                );
            }
            src_to_dst.push(dst_pc);
            is_data_payload[src_pc] = true;
            sizes[src_pc] = 0;
        }
        src_pc += 1;
    }
    src_to_dst.push(dst_pc); // sentinel for end-of-code

    let mut out: Vec<Inst> = Vec::with_capacity(dst_pc as usize);
    let mut out_lines: Vec<u32> = Vec::with_capacity(dst_pc as usize);
    let mut max_temp_bump: u8 = 0;
    let mut src_pc = 0;
    while src_pc < n_src {
        if is_data_payload[src_pc] {
            src_pc += 1;
            continue;
        }
        let raw = raw_code[src_pc];
        let p = PucInst::decode(raw);
        let line = lines.get(src_pc).copied().unwrap_or(0);
        let pre_emit = out.len();
        // Hand the SETLIST-C=0 path the following raw word so it can
        // populate the ExtraArg payload directly.
        let payload = if p.op == P_SETLIST && p.c == 0 {
            Some(raw_code[src_pc + 1])
        } else {
            None
        };
        translate_one(
            &mut out,
            src_pc,
            p,
            &src_to_dst,
            payload,
            &mut max_temp_bump,
        )?;
        let emitted = out.len() - pre_emit;
        if emitted as u32 != sizes[src_pc] {
            return Err(format!(
                "PUC 5.2 translator: src_pc {src_pc} expected {} emits, got {}",
                sizes[src_pc], emitted
            ));
        }
        for _ in 0..emitted {
            out_lines.push(line);
        }
        src_pc += 1;
    }
    Ok((out, out_lines, max_temp_bump))
}

fn src_size(p: PucInst) -> Result<u32, String> {
    match p.op {
        P_JMP if p.a != 0 => Ok(2),     // Close + Jmp
        P_SETLIST if p.c == 0 => Ok(2), // SetList + ExtraArg payload
        // arith with K on B / C lowers to `LoadK tmp; OP a tmp c/k`. K on
        // both adds a second LoadK for the C operand.
        P_ADD | P_SUB | P_MUL | P_DIV | P_MOD | P_POW => {
            let b_isk = (p.b & RK_BIT) != 0;
            let c_isk = (p.c & RK_BIT) != 0;
            match (b_isk, c_isk) {
                (false, _) => Ok(1),    // C-K maps to luna's k flag, no temp
                (true, false) => Ok(2), // LoadK tmp K[B]; OP a tmp c
                (true, true) => Ok(3),  // LoadK tmpB; LoadK tmpC; OP a tmpB tmpC
            }
        }
        // Eq/Lt/Le with K on B or C lowers via tmp(s). luna's Eq/Lt/Le take
        // R[A] R[B] with the k bit reserved for the truth sense, so K can't
        // be folded into either operand — every K becomes a LoadK tmp.
        P_EQ | P_LT | P_LE => {
            let b_isk = (p.b & RK_BIT) != 0;
            let c_isk = (p.c & RK_BIT) != 0;
            Ok(1 + (b_isk as u32) + (c_isk as u32))
        }
        // GETTABUP with a *register* key (PUC's C is R, not K) lowers to
        // `GetUpval tmp B; GetTable a tmp c`. K-string keys stay 1:1.
        P_GETTABUP => {
            let c_isk = (p.c & RK_BIT) != 0;
            Ok(if c_isk { 1 } else { 2 })
        }
        _ => Ok(1),
    }
}

/// Map a PUC 5.2 source PC + sBx offset to a luna sJ jump offset. The
/// `src_pc + 1 + sBx` arithmetic comes from PUC's `dojump`; the +1
/// reflects the fact that PUC bumps PC before adding sBx. luna's `Jmp`
/// fires after the dispatcher has already advanced `pc` past the Jmp
/// itself, so the same +1 applies.
fn remap_jump(src_pc: usize, sbx: i32, src_to_dst: &[u32]) -> Result<i32, String> {
    let target_src = (src_pc as i32) + 1 + sbx;
    if target_src < 0 || target_src as usize >= src_to_dst.len() {
        return Err(format!(
            "PUC 5.2 translator: jump target {target_src} out of range"
        ));
    }
    let target_dst = src_to_dst[target_src as usize] as i32;
    // Find the dst position of this src op so we can compute the offset
    // from the Jmp we're about to emit. When the JMP expanded to
    // `Close; Jmp`, the Jmp itself lives at `src_to_dst[src_pc] + 1`.
    let here_dst = src_to_dst[src_pc] as i32;
    // We don't know without context which slot inside the expansion
    // holds the Jmp. The two callers below pass `here_dst` adjusted
    // accordingly; this fn just computes the delta.
    Ok(target_dst - (here_dst + 1))
}

fn translate_one(
    out: &mut Vec<Inst>,
    src_pc: usize,
    p: PucInst,
    src_to_dst: &[u32],
    setlist_payload: Option<u32>,
    max_temp_bump: &mut u8,
) -> Result<(), String> {
    let a = p.a;
    match p.op {
        // R(A) := R(B)
        P_MOVE => out.push(Inst::iabc(Op::Move, a, p.b, 0, false)),
        // R(A) := Kst(Bx)
        P_LOADK => out.push(Inst::iabx(Op::LoadK, a, p.bx())),
        // R(A) := Kst(extra arg) — next op MUST be EXTRAARG (we leave the
        // following emitted ExtraArg in place when we hit P_EXTRAARG).
        P_LOADKX => out.push(Inst::iabc(Op::LoadKx, a, 0, 0, false)),
        // R(A) := (Bool)B; if (C) pc++
        P_LOADBOOL => {
            if p.b == 0 && p.c == 0 {
                out.push(Inst::iabc(Op::LoadFalse, a, 0, 0, false));
            } else if p.b == 0 && p.c != 0 {
                out.push(Inst::iabc(Op::LFalseSkip, a, 0, 0, false));
            } else if p.b != 0 && p.c == 0 {
                out.push(Inst::iabc(Op::LoadTrue, a, 0, 0, false));
            } else {
                // LOADBOOL A 1 1 is structurally legal but PUC's
                // compiler never emits it (the "skip" form is only
                // used by the false-then-true comparison pattern).
                // luna has no LTrueSkip; reject loud rather than
                // silently miscompile.
                return Err("PUC 5.2 translator: LOADBOOL A 1 1 unsupported \
                     (no LTrueSkip in luna)"
                    .to_string());
            }
        }
        // R(A), R(A+1), ..., R(A+B) := nil. 5.2 range is inclusive of A+B;
        // luna's LoadNil clears `R[A..A+B]` (5.4-style — also inclusive
        // of A+B), so the B field maps 1:1.
        P_LOADNIL => out.push(Inst::iabc(Op::LoadNil, a, p.b, 0, false)),
        P_GETUPVAL => out.push(Inst::iabc(Op::GetUpval, a, p.b, 0, false)),
        // R(A) := UpValue[B][RK(C)]
        P_GETTABUP => {
            let (c_idx, c_isk) = decode_rk(p.c)?;
            if c_isk {
                // K-string key — luna's GetTabUp handles this 1:1.
                out.push(Inst::iabc(Op::GetTabUp, a, p.b, c_idx as u32, false));
            } else {
                // Register-key case: PUC 5.2 emits `R[A] := UpValue[B][R[C]]`
                // for dynamic lookups through a captured table. luna's
                // GetTabUp only accepts a K-string key, so lower to:
                // `GetUpval tmp B; GetTable A tmp C`. Use `tmp = max(a, c) + 1`
                // to avoid clobbering either operand before the GetTable
                // reads them. PUC's compiler reserves the slot for this
                // pattern's intermediate so `max_stack` is already close;
                // the post-pass bump tops it off.
                let tmp = a.max(c_idx as u32) + 1;
                if tmp > 0xFF {
                    return Err(format!(
                        "PUC 5.2 translator: GETTABUP register-key lowering: \
                         temp register {tmp} exceeds 255 (src_pc {src_pc})"
                    ));
                }
                *max_temp_bump = (*max_temp_bump).max(tmp as u8 + 1);
                out.push(Inst::iabc(Op::GetUpval, tmp, p.b, 0, false));
                out.push(Inst::iabc(Op::GetTable, a, tmp, c_idx as u32, false));
            }
        }
        // R(A) := R(B)[RK(C)]
        P_GETTABLE => {
            let (c_idx, c_isk) = decode_rk(p.c)?;
            out.push(Inst::iabc(Op::GetTable, a, p.b, c_idx as u32, c_isk));
        }
        // UpValue[A][RK(B)] := RK(C). luna's SetTabUp uses upval A,
        // K-string B, R/K C with the k flag.
        P_SETTABUP => {
            let (b_idx, b_isk) = decode_rk(p.b)?;
            let (c_idx, c_isk) = decode_rk(p.c)?;
            if !b_isk {
                return Err(
                    "PUC 5.2 translator: SETTABUP with register name not supported".to_string(),
                );
            }
            // luna's SetTabUp: a = upval, b = k-string idx, c = R/K
            // payload, k flag = C is K.
            out.push(Inst::iabc(
                Op::SetTabUp,
                a,
                b_idx as u32,
                c_idx as u32,
                c_isk,
            ));
        }
        P_SETUPVAL => out.push(Inst::iabc(Op::SetUpval, a, p.b, 0, false)),
        // R(A)[RK(B)] := RK(C)
        P_SETTABLE => {
            let (b_idx, b_isk) = decode_rk(p.b)?;
            let (c_idx, c_isk) = decode_rk(p.c)?;
            // luna's SetTable takes register R[A], R/K key in B with
            // its k bit, R/K val in C. luna packs both K flags into one
            // `k` bit — it can encode val-K but not key-K independently.
            // PUC's 5.2 compiler emits SETTABLE with RK on both sides;
            // when the key is K we lower to: LoadK tmp; SetTable A tmp C.
            // For first cut, support key-as-K when val-as-K matches, else
            // require key-as-R.
            if b_isk {
                // Lower to a SetField-style by promoting the K key. luna
                // has SetField for K-string keys — use it.
                out.push(Inst::iabc(
                    Op::SetField,
                    a,
                    b_idx as u32,
                    c_idx as u32,
                    c_isk,
                ));
            } else {
                out.push(Inst::iabc(
                    Op::SetTable,
                    a,
                    b_idx as u32,
                    c_idx as u32,
                    c_isk,
                ));
            }
        }
        // R(A) := {} (size = B,C). B = array hint, C = hash hint, both
        // floating-point bytes per `luaO_int2fb`.
        P_NEWTABLE => {
            let arr = fb_to_hint_u8(p.b);
            let hsh = fb_to_hint_u8(p.c);
            out.push(Inst::iabc(Op::NewTable, a, arr, hsh, false));
        }
        // R(A+1) := R(B); R(A) := R(B)[RK(C)]
        P_SELF => {
            let (c_idx, c_isk) = decode_rk(p.c)?;
            out.push(Inst::iabc(Op::SelfOp, a, p.b, c_idx as u32, c_isk));
        }
        // Arithmetic: R(A) := RK(B) op RK(C). luna's Add/Sub/etc. only
        // support val-as-K via the `k` bit (C operand). PUC 5.2 places
        // either or both operands in K (e.g. `1 - x` or `1 + 2` if not
        // const-folded), so we materialize K-on-LHS via the shared
        // `lower_k_via_tmp` helper; K-on-both expands to two LoadKs +
        // reg-reg op. Sizes here must match `src_size`.
        op @ (P_ADD | P_SUB | P_MUL | P_DIV | P_MOD | P_POW) => {
            let (b_idx, b_isk) = decode_rk(p.b)?;
            let (c_idx, c_isk) = decode_rk(p.c)?;
            let luna_op = match op {
                P_ADD => Op::Add,
                P_SUB => Op::Sub,
                P_MUL => Op::Mul,
                P_DIV => Op::Div,
                P_MOD => Op::Mod,
                P_POW => Op::Pow,
                _ => unreachable!(),
            };
            match (b_isk, c_isk) {
                // Common case: R op R, or R op K (luna's `k` bit fits).
                (false, _) => {
                    out.push(Inst::iabc(luna_op, a, b_idx as u32, c_idx as u32, c_isk));
                }
                // K op R: LoadK tmp K[B]; OP a tmp c. tmp must clear
                // both `a` and `c` so the OP's reg reads are intact
                // when it writes `a`.
                (true, false) => {
                    let tmp = a.max(c_idx as u32) + 1;
                    let pair = super::lower_k_via_tmp(
                        luna_op,
                        a,
                        b_idx as u32,
                        c_idx as u32,
                        false,
                        tmp,
                        max_temp_bump,
                    )?;
                    out.push(pair[0]);
                    out.push(pair[1]);
                }
                // K op K: PUC's compiler normally const-folds these, but
                // a stripped or hand-crafted chunk can present them.
                // LoadK tmpB K[B]; LoadK tmpC K[C]; OP a tmpB tmpC.
                (true, true) => {
                    let tmp_b = a + 1;
                    let tmp_c = a + 2;
                    if tmp_c > 0xFF {
                        return Err(format!(
                            "PUC 5.2 translator: arith K-on-both lowering: \
                             temp register {tmp_c} exceeds 255 (src_pc {src_pc})"
                        ));
                    }
                    if (b_idx as u32) > 0x1FFFF || (c_idx as u32) > 0x1FFFF {
                        return Err(format!(
                            "PUC 5.2 translator: arith K-on-both lowering: \
                             K-pool index exceeds 17-bit Bx (src_pc {src_pc})"
                        ));
                    }
                    *max_temp_bump = (*max_temp_bump).max(tmp_c as u8 + 1);
                    out.push(Inst::iabx(Op::LoadK, tmp_b, b_idx as u32));
                    out.push(Inst::iabx(Op::LoadK, tmp_c, c_idx as u32));
                    out.push(Inst::iabc(luna_op, a, tmp_b, tmp_c, false));
                }
            }
        }
        // Unary: R(A) := op R(B)
        P_UNM => out.push(Inst::iabc(Op::Unm, a, p.b, 0, false)),
        P_NOT => out.push(Inst::iabc(Op::Not, a, p.b, 0, false)),
        P_LEN => out.push(Inst::iabc(Op::Len, a, p.b, 0, false)),
        // R(A) := R(B) .. R(B+1) .. ... .. R(C). luna's Concat has the
        // 5.4 shape `R(A) .. R(A+B-1)` — one start register, one count.
        // Translate by remapping: A=A, B=C-B+1, C=0.
        P_CONCAT => {
            if p.c < p.b {
                return Err("PUC 5.2 translator: CONCAT with C < B".to_string());
            }
            let count = p.c - p.b + 1;
            // luna's Concat treats A as the source start (where the
            // first operand lives) — PUC has the same convention since
            // both expect A == B for the typical compiler output.
            if p.b != p.a {
                // PUC 5.2 always emits B == A for CONCAT (per
                // luaK_codeconcat), but be defensive.
                return Err("PUC 5.2 translator: CONCAT with A != B not supported".to_string());
            }
            out.push(Inst::iabc(Op::Concat, a, count, 0, false));
        }
        // pc += sBx; if (A) close all upvalues >= R(A - 1). When A != 0
        // we need to emit a Close before the Jmp.
        P_JMP => {
            if a != 0 {
                // Close R[A-1..]
                out.push(Inst::iabc(Op::Close, a - 1, 0, 0, false));
            }
            let sj = remap_jump_for_jmp(src_pc, p.sbx(), src_to_dst, a != 0)?;
            out.push(Inst::isj(Op::Jmp, sj));
        }
        // Comparison: if ((RK(B) op RK(C)) ~= A) then pc++. luna's
        // Eq/Lt/Le take registers R[A], R[B] with the `k` bit reserved
        // for the truth-sense (PUC's `A` field). Constant operands can't
        // be folded into either side, so each K becomes a `LoadK tmp` emit.
        // Sizes here must match `src_size`.
        op @ (P_EQ | P_LT | P_LE) => {
            let (b_idx, b_isk) = decode_rk(p.b)?;
            let (c_idx, c_isk) = decode_rk(p.c)?;
            let luna_op = match op {
                P_EQ => Op::Eq,
                P_LT => Op::Lt,
                P_LE => Op::Le,
                _ => unreachable!(),
            };
            // PUC's `A` is the "expected truth" bit (skip if cmp != A).
            // luna's `k` bit means the same thing. Map A->k directly.
            let k = a != 0;
            // Materialize K operand(s) into temps that clear the existing
            // register payload, then emit a reg-reg compare. Temp slots
            // grow upward from `max(b_reg, c_reg) + 1`; when only one
            // side is K, the temp goes one above the live reg.
            let live_max = match (b_isk, c_isk) {
                (false, false) => 0, // unreachable in this branch (no K)
                (true, false) => c_idx as u32,
                (false, true) => b_idx as u32,
                (true, true) => 0,
            };
            let mut next_tmp = live_max + 1;
            let mut alloc_tmp = || -> Result<u8, String> {
                let t = next_tmp;
                if t > 0xFF {
                    return Err(format!(
                        "PUC 5.2 translator: comparison K lowering: \
                         temp register {t} exceeds 255 (src_pc {src_pc})"
                    ));
                }
                next_tmp += 1;
                *max_temp_bump = (*max_temp_bump).max(t as u8 + 1);
                Ok(t as u8)
            };
            let b_reg = if b_isk {
                let t = alloc_tmp()?;
                if (b_idx as u32) > 0x1FFFF {
                    return Err(format!(
                        "PUC 5.2 translator: comparison K lowering: \
                         B K-pool index {b_idx} exceeds 17-bit Bx (src_pc {src_pc})"
                    ));
                }
                out.push(Inst::iabx(Op::LoadK, t as u32, b_idx as u32));
                t as u32
            } else {
                b_idx as u32
            };
            let c_reg = if c_isk {
                let t = alloc_tmp()?;
                if (c_idx as u32) > 0x1FFFF {
                    return Err(format!(
                        "PUC 5.2 translator: comparison K lowering: \
                         C K-pool index {c_idx} exceeds 17-bit Bx (src_pc {src_pc})"
                    ));
                }
                out.push(Inst::iabx(Op::LoadK, t as u32, c_idx as u32));
                t as u32
            } else {
                c_idx as u32
            };
            out.push(Inst::iabc(luna_op, b_reg, c_reg, 0, k));
        }
        // if not (R(A) <=> C) then pc++. luna's Test reads R[A], k bit
        // is the sense. PUC's C is the expected truth; map C->k.
        P_TEST => out.push(Inst::iabc(Op::Test, a, 0, 0, p.c != 0)),
        // if (R(B) <=> C) then R(A) := R(B) else pc++
        P_TESTSET => out.push(Inst::iabc(Op::TestSet, a, p.b, 0, p.c != 0)),
        // R(A), ..., R(A+C-2) := R(A)(R(A+1), ..., R(A+B-1))
        P_CALL => out.push(Inst::iabc(Op::Call, a, p.b, p.c, false)),
        // return R(A)(R(A+1), ..., R(A+B-1))
        P_TAILCALL => out.push(Inst::iabc(Op::TailCall, a, p.b, p.c, false)),
        // return R(A), ..., R(A+B-2)
        P_RETURN => out.push(Inst::iabc(Op::Return, a, p.b, 0, false)),
        // numeric-for (sBx form on FORPREP / FORLOOP)
        P_FORPREP => {
            let sj = remap_jump(src_pc, p.sbx(), src_to_dst)?;
            // luna's ForPrep takes `A sBx` — re-encode the offset.
            out.push(Inst::iasbx(Op::ForPrep, a, sj));
        }
        P_FORLOOP => {
            let sj = remap_jump(src_pc, p.sbx(), src_to_dst)?;
            out.push(Inst::iasbx(Op::ForLoop, a, sj));
        }
        // R(A+3), ..., R(A+2+C) := R(A)(R(A+1), R(A+2))
        P_TFORCALL => out.push(Inst::iabc(Op::TForCall, a, 0, p.c, false)),
        // if R(A+1) ~= nil then { R(A) = R(A+1); pc += sBx }. luna's
        // TForLoop uses A pointing at the iterator triple's BASE (state
        // = A+1, ctrl = A+2 in luna's 5.4-style convention; ctrl ends
        // up at A+4 after TForCall stored results). 5.2's TFORLOOP_A
        // = TFORCALL_A + 2 because PUC's TFORLOOP reads the ctrl from
        // A+1. Remap A_luna = A_5_2 - 2, and re-encode sBx as bx (luna
        // negates bx at runtime).
        P_TFORLOOP => {
            if a < 2 {
                return Err(
                    "PUC 5.2 translator: TFORLOOP A < 2 (impossible per PUC convention)"
                        .to_string(),
                );
            }
            let a_luna = a - 2;
            // luna's TForLoop: `add_pc(-bx)` if ctrl != nil. 5.2's sBx
            // is a signed back-edge; convert to a positive bx by
            // negating, after remapping for translation drift.
            let sj = remap_jump(src_pc, p.sbx(), src_to_dst)?;
            // sj points forward at the body top from the next op; for
            // a back-edge this is negative. luna's bx = -offset.
            let bx_val = -sj;
            if bx_val < 0 {
                return Err(format!(
                    "PUC 5.2 translator: TFORLOOP forward jump (sj={sj}) not supported"
                ));
            }
            let bx = bx_val as u32;
            if bx > isa::MAX_BX {
                return Err(format!(
                    "PUC 5.2 translator: TFORLOOP back-edge {bx} > MAX_BX"
                ));
            }
            out.push(Inst::iabx(Op::TForLoop, a_luna, bx));
        }
        // R(A)[(C-1)*FPF+i] := R(A+i), 1 <= i <= B. PUC: when C == 0,
        // the next instruction holds the C value (luna uses the same
        // trick via the k bit + EXTRAARG). When C != 0, encode as a
        // single op.
        P_SETLIST => {
            let b = p.b;
            if p.c == 0 {
                // luna's SetList with k=true reads C from the following
                // ExtraArg's ax field. PUC stores the C in the next
                // *raw u32* (treated as a plain integer, not a packed
                // instruction). luna's ExtraArg `ax` is 25 bits — enough
                // for any realistic SETLIST block index (PUC caps the
                // block index near 2^24).
                let payload = setlist_payload.ok_or_else(|| {
                    "PUC 5.2 translator: SETLIST C=0 missing payload (internal bug)".to_string()
                })?;
                if payload > isa::MAX_AX {
                    return Err(format!(
                        "PUC 5.2 translator: SETLIST payload {payload} > luna MAX_AX"
                    ));
                }
                out.push(Inst::iabc(Op::SetList, a, b, 0, true));
                out.push(Inst::iax(Op::ExtraArg, payload));
            } else {
                out.push(Inst::iabc(Op::SetList, a, b, p.c, false));
            }
        }
        // R(A) := closure(KPROTO[Bx])
        P_CLOSURE => out.push(Inst::iabx(Op::Closure, a, p.bx())),
        // R(A), R(A+1), ..., R(A+B-2) = vararg
        P_VARARG => out.push(Inst::iabc(Op::Vararg, a, p.b, 0, false)),
        // extra (larger) argument for previous opcode. PUC emits this
        // ONLY directly after LOADKX (LOADBOOL/SETLIST trail it too in
        // theory, but PUC's emitter uses the LOADKX path). luna's
        // ExtraArg works the same way. Re-emit as ExtraArg with the
        // full 26-bit Ax field — luna's Ax is 25 bits, so values >
        // MAX_AX are rejected (a stripped + huge-const-pool chunk
        // could in theory tickle this but PUC's MAXARG_Ax caps near
        // 2^26).
        P_EXTRAARG => {
            if p.ax() > isa::MAX_AX {
                return Err(format!(
                    "PUC 5.2 translator: EXTRAARG ax={} > luna MAX_AX",
                    p.ax()
                ));
            }
            out.push(Inst::iax(Op::ExtraArg, p.ax()));
        }
        op => return Err(format!("PUC 5.2 translator: unhandled opcode {op}")),
    }
    Ok(())
}

/// Variant of `remap_jump` that accounts for `OP_JMP A sBx` having
/// expanded into `Close; Jmp` when `had_close` is true. The Jmp itself
/// lives at `src_to_dst[src_pc] + 1` in that case, so the +1 in the
/// `target_dst - (here_dst + 1)` arithmetic is replaced by +2.
fn remap_jump_for_jmp(
    src_pc: usize,
    sbx: i32,
    src_to_dst: &[u32],
    had_close: bool,
) -> Result<i32, String> {
    let target_src = (src_pc as i32) + 1 + sbx;
    if target_src < 0 || target_src as usize >= src_to_dst.len() {
        return Err(format!(
            "PUC 5.2 translator: jump target {target_src} out of range"
        ));
    }
    let target_dst = src_to_dst[target_src as usize] as i32;
    let jmp_dst = src_to_dst[src_pc] as i32 + if had_close { 1 } else { 0 };
    Ok(target_dst - (jmp_dst + 1))
}

#[cfg(test)]
#[allow(clippy::identity_op, clippy::erasing_op)]
// Bitfield-construction helpers in test fixtures spell out every shift
// even when the value is 0, to document the PUC opcode encoding layout.
// `(0u32 << 14)` is clearer-as-spec than dropping the term.
mod tests {
    use super::*;

    #[test]
    fn fb2int_known_values() {
        // Identity for x < 8
        assert_eq!(fb2int(0), 0);
        assert_eq!(fb2int(7), 7);
        // luaO_int2fb(8) = (1<<3) | 0 = 8 → fb2int(8) = ((8&7)+8) << 0 = 8
        assert_eq!(fb2int(8), 8);
        // luaO_int2fb(16) = (2<<3) | 0 = 16 → fb2int(16) = 16
        assert_eq!(fb2int(16), 16);
        // luaO_int2fb(20) = (2<<3) | 2 = 18 → fb2int(18) = ((18&7)+8)<<1 = 20
        assert_eq!(fb2int(18), 20);
    }

    #[test]
    fn decode_inst_layout() {
        // Hand-encoded OP_MOVE A=3 B=5 (no C, no K). 6-bit op | 8-bit A
        // | 9-bit C | 9-bit B.
        let raw: u32 = (P_MOVE as u32) | (3u32 << 6) | (0u32 << 14) | (5u32 << 23);
        let p = PucInst::decode(raw);
        assert_eq!(p.op, P_MOVE);
        assert_eq!(p.a, 3);
        assert_eq!(p.b, 5);
        assert_eq!(p.c, 0);
    }

    #[test]
    fn loadbool_lowering() {
        let mut out = Vec::new();
        // LOADBOOL A=2 B=0 C=0 → LoadFalse
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_LOADBOOL,
                a: 2,
                b: 0,
                c: 0,
            },
            &[0, 1],
            None,
            &mut 0,
        )
        .unwrap();
        assert_eq!(out[0].op(), Op::LoadFalse);
        assert_eq!(out[0].a(), 2);

        out.clear();
        // LOADBOOL A=3 B=0 C=1 → LFalseSkip
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_LOADBOOL,
                a: 3,
                b: 0,
                c: 1,
            },
            &[0, 1],
            None,
            &mut 0,
        )
        .unwrap();
        assert_eq!(out[0].op(), Op::LFalseSkip);

        out.clear();
        // LOADBOOL A=4 B=1 C=0 → LoadTrue
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_LOADBOOL,
                a: 4,
                b: 1,
                c: 0,
            },
            &[0, 1],
            None,
            &mut 0,
        )
        .unwrap();
        assert_eq!(out[0].op(), Op::LoadTrue);
    }

    // -- Wave 2 punt recovery: arith K-on-LHS / K-on-both ---------------

    #[test]
    fn arith_k_on_rhs_emits_one_inst_with_k_flag() {
        // R[2] := R[3] + K[5]  →  Add a=2 b=3 c=5 k=true (single emit).
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_ADD,
                a: 2,
                b: 3,
                c: 5 | RK_BIT,
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op(), Op::Add);
        assert_eq!(out[0].a(), 2);
        assert_eq!(out[0].b(), 3);
        assert_eq!(out[0].c(), 5);
        assert!(out[0].k());
        assert_eq!(bump, 0, "no temp needed when only C is K");
    }

    #[test]
    fn arith_k_on_lhs_lowers_via_loadk_tmp() {
        // R[1] := K[7] - R[2]  →  LoadK tmp K[7]; Sub 1 tmp 2.
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_SUB,
                a: 1,
                b: 7 | RK_BIT,
                c: 2,
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].op(), Op::LoadK);
        let tmp = out[0].a();
        assert!(tmp > 2, "tmp ({tmp}) must clear both a=1 and c=2");
        assert_eq!(out[0].bx(), 7);
        assert_eq!(out[1].op(), Op::Sub);
        assert_eq!(out[1].a(), 1);
        assert_eq!(out[1].b(), tmp);
        assert_eq!(out[1].c(), 2);
        assert!(!out[1].k(), "k flag stays clear when lowering K-on-LHS");
        assert_eq!(bump, tmp as u8 + 1);
    }

    #[test]
    fn arith_k_on_both_lowers_via_two_loadks() {
        // R[0] := K[3] * K[4]  →  LoadK tmpB K[3]; LoadK tmpC K[4]; Mul 0 tmpB tmpC.
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_MUL,
                a: 0,
                b: 3 | RK_BIT,
                c: 4 | RK_BIT,
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].op(), Op::LoadK);
        assert_eq!(out[0].bx(), 3);
        assert_eq!(out[1].op(), Op::LoadK);
        assert_eq!(out[1].bx(), 4);
        assert_eq!(out[2].op(), Op::Mul);
        assert_eq!(out[2].a(), 0);
        assert_eq!(out[2].b(), out[0].a());
        assert_eq!(out[2].c(), out[1].a());
        assert_ne!(out[0].a(), out[1].a(), "tmpB and tmpC must differ");
        assert!(bump >= 3, "frame must grow to hold two new temps");
    }

    #[test]
    fn arith_all_six_ops_round_trip_k_on_lhs() {
        // Sanity that ADD/SUB/MUL/DIV/MOD/POW all reach the same lowering arm.
        for (puc_op, luna_op) in [
            (P_ADD, Op::Add),
            (P_SUB, Op::Sub),
            (P_MUL, Op::Mul),
            (P_DIV, Op::Div),
            (P_MOD, Op::Mod),
            (P_POW, Op::Pow),
        ] {
            let mut out = Vec::new();
            translate_one(
                &mut out,
                0,
                PucInst {
                    op: puc_op,
                    a: 0,
                    b: 1 | RK_BIT,
                    c: 2,
                },
                &[0, 1],
                None,
                &mut 0,
            )
            .unwrap();
            assert_eq!(out[0].op(), Op::LoadK);
            assert_eq!(out[1].op(), luna_op);
        }
    }

    // -- Wave 2 punt recovery: EQ/LT/LE with constant operands -----------

    #[test]
    fn eq_with_k_on_rhs_lowers_via_loadk_tmp() {
        // (R[3] == K[1]) ~= 1  →  LoadK tmp K[1]; Eq R[3] tmp k=true.
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_EQ,
                a: 1, // truth bit
                b: 3,
                c: 1 | RK_BIT,
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].op(), Op::LoadK);
        let tmp = out[0].a();
        assert!(tmp > 3, "tmp must clear live reg b=3");
        assert_eq!(out[0].bx(), 1);
        assert_eq!(out[1].op(), Op::Eq);
        assert_eq!(out[1].a(), 3);
        assert_eq!(out[1].b(), tmp);
        assert!(out[1].k(), "PUC A=1 maps to luna k=true");
        assert_eq!(bump, tmp as u8 + 1);
    }

    #[test]
    fn lt_with_k_on_lhs_lowers_via_loadk_tmp() {
        // (K[2] < R[5]) ~= 0  →  LoadK tmp K[2]; Lt tmp R[5] k=false.
        let mut out = Vec::new();
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_LT,
                a: 0,
                b: 2 | RK_BIT,
                c: 5,
            },
            &[0, 1],
            None,
            &mut 0,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].op(), Op::LoadK);
        let tmp = out[0].a();
        assert!(tmp > 5, "tmp must clear live reg c=5");
        assert_eq!(out[1].op(), Op::Lt);
        assert_eq!(out[1].a(), tmp);
        assert_eq!(out[1].b(), 5);
        assert!(!out[1].k());
    }

    #[test]
    fn le_with_k_on_both_lowers_via_two_loadks() {
        // (K[0] <= K[2]) ~= 1  →  LoadK tmpB K[0]; LoadK tmpC K[2]; Le tmpB tmpC k=true.
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_LE,
                a: 1,
                b: 0 | RK_BIT,
                c: 2 | RK_BIT,
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].op(), Op::LoadK);
        assert_eq!(out[0].bx(), 0);
        assert_eq!(out[1].op(), Op::LoadK);
        assert_eq!(out[1].bx(), 2);
        assert_eq!(out[2].op(), Op::Le);
        assert_eq!(out[2].a(), out[0].a());
        assert_eq!(out[2].b(), out[1].a());
        assert!(out[2].k());
        assert_ne!(out[0].a(), out[1].a());
        assert!(bump >= 2);
    }

    // -- Wave 2 punt recovery: GETTABUP with register key ---------------

    #[test]
    fn gettabup_register_key_lowers_to_getupval_gettable() {
        // R[1] := UpValue[0][R[2]]  →  GetUpval tmp 0; GetTable 1 tmp 2.
        let mut out = Vec::new();
        let mut bump: u8 = 0;
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_GETTABUP,
                a: 1,
                b: 0,
                c: 2, // register, RK_BIT not set
            },
            &[0, 1],
            None,
            &mut bump,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].op(), Op::GetUpval);
        let tmp = out[0].a();
        assert!(tmp > 2, "tmp must clear a=1 and c=2");
        assert_eq!(out[0].b(), 0, "GetUpval reads upval slot from B");
        assert_eq!(out[1].op(), Op::GetTable);
        assert_eq!(out[1].a(), 1);
        assert_eq!(out[1].b(), tmp);
        assert_eq!(out[1].c(), 2);
        assert_eq!(bump, tmp as u8 + 1);
    }

    #[test]
    fn gettabup_constant_key_stays_one_inst() {
        // Sanity: K-string key path unchanged (still 1:1).
        let mut out = Vec::new();
        translate_one(
            &mut out,
            0,
            PucInst {
                op: P_GETTABUP,
                a: 1,
                b: 0,
                c: 4 | RK_BIT,
            },
            &[0, 1],
            None,
            &mut 0,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].op(), Op::GetTabUp);
    }

    // -- src_size mirrors translate_one for all Wave 2 cases ------------

    #[test]
    fn src_size_matches_emits_for_wave2_punts() {
        let cases = [
            // (puc_op, b, c, expected_size, label)
            (P_ADD, 1, 2, 1, "R + R"),
            (P_ADD, 1, 2 | RK_BIT, 1, "R + K (fits luna k bit)"),
            (P_SUB, 1 | RK_BIT, 2, 2, "K - R"),
            (P_MUL, 1 | RK_BIT, 2 | RK_BIT, 3, "K * K"),
            (P_EQ, 1, 2, 1, "R == R"),
            (P_LT, 1, 2 | RK_BIT, 2, "R < K"),
            (P_LE, 1 | RK_BIT, 2, 2, "K <= R"),
            (P_EQ, 1 | RK_BIT, 2 | RK_BIT, 3, "K == K"),
            (P_GETTABUP, 0, 4 | RK_BIT, 1, "GETTABUP K key"),
            (P_GETTABUP, 0, 2, 2, "GETTABUP R key"),
        ];
        for (op, b, c, expected, label) in cases {
            let p = PucInst { op, a: 0, b, c };
            let size = src_size(p).unwrap();
            assert_eq!(size, expected, "src_size mismatch: {label}");
        }
    }
}
