//! `string.dump` / chunk undump.
//!
//! The header mirrors PUC's per-version layout (calls.lua's `headformat`
//! round-trips with the matching component values for whichever dialect is
//! running), so a corrupted-header test rejects luna's chunks the same way
//! PUC would. The body that follows is luna-specific — luna's VM cannot
//! execute PUC bytecode (different opcode encoding, register conventions,
//! etc.), so the chunk only needs to round-trip within luna. `strip` drops
//! debug names (local-variable records and upvalue names); line info is
//! always kept because the VM indexes it for error positions.

use crate::runtime::Value;
use crate::runtime::function::{LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::version::LuaVersion;

/// PUC 5.5 binary-chunk header (40 bytes), byte-for-byte:
///
/// 1. `\x1bLua` (4) — signature
/// 2. `0x55` (1)   — version
/// 3. `0x00` (1)   — format
/// 4. `\x19\x93\r\n\x1a\n` (6) — luac binary check
/// 5. `4` (1)      — sizeof(int)
/// 6. int `-0x5678`        (4)  — sanity check (le)
/// 7. `4` (1)      — sizeof(Instruction)
/// 8. inst `0x12345678`    (4)  — sanity check (le)
/// 9. `8` (1)      — sizeof(lua_Integer)
/// 10. int `-0x5678`       (8)  — sanity check (le)
/// 11. `8` (1)     — sizeof(lua_Number)
/// 12. float `-370.5`      (8)  — sanity check (le)
const HEADER_55: &[u8] = &[
    0x1b, b'L', b'u', b'a',
    0x55, 0x00,
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
    4,
    0x88, 0xa9, 0xff, 0xff,
    4,
    0x78, 0x56, 0x34, 0x12,
    8,
    0x88, 0xa9, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    8,
    0, 0, 0, 0, 0, 0x28, 0x77, 0xc0,
];

/// PUC 5.4 binary-chunk header (31 bytes), per `ldump.c DumpHeader`:
/// signature + 0x54 + format + LUAC_DATA + sizeof(Instruction) +
/// sizeof(lua_Integer) + sizeof(lua_Number) + LUAC_INT (0x5678) +
/// LUAC_NUM (370.5). calls.lua :395 packs the first 15 bytes plus an
/// `(jn)` unpack of the next 16 to lock these values in.
const HEADER_54: &[u8] = &[
    0x1b, b'L', b'u', b'a',
    0x54, 0x00,
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
    4,                                              // sizeof(Instruction)
    8,                                              // sizeof(lua_Integer)
    8,                                              // sizeof(lua_Number)
    0x78, 0x56, 0, 0, 0, 0, 0, 0,                   // LUAC_INT = 0x5678
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40,                // LUAC_NUM = 370.5
];

/// PUC 5.3 binary-chunk header (33 bytes), per 5.3 `ldump.c DumpHeader`:
/// signature + 0x53 + format + LUAC_DATA + sizeof(int) + sizeof(size_t) +
/// sizeof(Instruction) + sizeof(lua_Integer) + sizeof(lua_Number) +
/// LUAC_INT (0x5678) + LUAC_NUM (370.5). calls.lua :381 packs the first
/// 25 bytes; the trailing 8-byte LUAC_NUM is not locked by an assertion
/// but the loader still expects it.
const HEADER_53: &[u8] = &[
    0x1b, b'L', b'u', b'a',
    0x53, 0x00,
    0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
    4,                                              // sizeof(int)
    8,                                              // sizeof(size_t)
    4,                                              // sizeof(Instruction)
    8,                                              // sizeof(lua_Integer)
    8,                                              // sizeof(lua_Number)
    0x78, 0x56, 0, 0, 0, 0, 0, 0,                   // LUAC_INT = 0x5678
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40,                // LUAC_NUM = 370.5
];

fn header_for(version: LuaVersion) -> &'static [u8] {
    match version {
        LuaVersion::Lua53 => HEADER_53,
        LuaVersion::Lua54 => HEADER_54,
        // 5.1 / 5.2 calls.lua does not test binary-chunk header bytes, so
        // route them through the 5.5 layout (luna's own dump round-trips
        // either way, and PUC 5.1/5.2 chunks aren't loadable into luna).
        _ => HEADER_55,
    }
}

/// luna's body-format tag, written immediately after the PUC header. PUC's
/// loader would reach this byte expecting the number of upvalues; we use a
/// non-PUC sentinel so an accidental cross-load (luna chunk into PUC, or
/// vice-versa) errors cleanly rather than misinterpreting bytes.
const BODY_TAG: &[u8] = b"\x00LunaV1\x00";

// ---- writer ----

fn w_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn w_bytes(out: &mut Vec<u8>, b: &[u8]) {
    w_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}

fn w_const(out: &mut Vec<u8>, v: Value) {
    match v {
        Value::Nil => out.push(0),
        Value::Bool(false) => out.push(1),
        Value::Bool(true) => out.push(2),
        Value::Int(i) => {
            out.push(3);
            out.extend_from_slice(&i.to_le_bytes());
        }
        Value::Float(f) => {
            out.push(4);
            out.extend_from_slice(&f.to_bits().to_le_bytes());
        }
        Value::Str(s) => {
            out.push(5);
            w_bytes(out, s.as_bytes());
        }
        // A constant table can only hold the above (the compiler never emits
        // table/function constants); anything else is a bug.
        other => unreachable!("non-serialisable constant: {}", other.type_name()),
    }
}

fn w_proto(out: &mut Vec<u8>, p: &Proto, strip: bool, parent_source: Option<&[u8]>) {
    out.push(p.num_params);
    out.push(p.is_vararg as u8);
    out.push(p.max_stack);
    w_u32(out, p.line_defined);
    w_u32(out, p.last_line_defined);
    // PUC `DumpFunction` (ldump.c) writes an empty source when stripping OR
    // when this proto shares its parent's source: the loader propagates the
    // parent's source down on the way up, so duplicating it bloats the dump
    // and (more importantly) lets calls.lua's `:556` reuse test see fewer
    // copies of a shared `<const>` string in the byte stream.
    let source = p.source.as_bytes();
    let inherits = parent_source == Some(source);
    w_bytes(out, if strip || inherits { b"" } else { source });

    w_u32(out, p.code.len() as u32);
    for inst in p.code.iter() {
        w_u32(out, inst.0);
    }
    // per-instruction line info is dropped when stripping (PUC lineinfo); the
    // VM tolerates an empty table (positions fall back to line 0 / -1).
    let lines: &[u32] = if strip { &[] } else { &p.lines };
    w_u32(out, lines.len() as u32);
    for &ln in lines.iter() {
        w_u32(out, ln);
    }

    w_u32(out, p.consts.len() as u32);
    for &k in p.consts.iter() {
        w_const(out, k);
    }

    w_u32(out, p.upvals.len() as u32);
    for u in p.upvals.iter() {
        out.push(u.in_stack as u8);
        out.push(u.index);
        out.push(u.read_only as u8);
        w_bytes(out, if strip { b"" } else { u.name.as_bytes() });
    }

    w_u32(out, p.protos.len() as u32);
    for sub in p.protos.iter() {
        w_proto(out, sub, strip, Some(source));
    }

    if strip {
        w_u32(out, 0);
    } else {
        w_u32(out, p.locvars.len() as u32);
        for lv in p.locvars.iter() {
            w_bytes(out, lv.name.as_bytes());
            w_u32(out, lv.reg);
            w_u32(out, lv.start_pc);
            w_u32(out, lv.end_pc);
        }
    }
}

/// Serialise a function prototype to a binary chunk: the PUC header for the
/// running dialect, a luna body tag, then the luna body.
pub fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Vec<u8> {
    let header = header_for(version);
    let mut out = Vec::with_capacity(header.len() + BODY_TAG.len() + proto.code.len() * 4);
    out.extend_from_slice(header);
    out.extend_from_slice(BODY_TAG);
    w_proto(&mut out, proto, strip, None);
    out
}

/// True when `bytes` is a luna binary chunk (so `load` should undump, not
/// parse). Only the escape byte is needed to disambiguate from source.
pub fn is_binary_chunk(bytes: &[u8]) -> bool {
    bytes.first() == Some(&0x1b)
}

// ---- reader ----

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.p.checked_add(n).ok_or("truncated chunk")?;
        let slice = self.b.get(self.p..end).ok_or("truncated chunk")?;
        self.p = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        self.take(n)
    }
}

fn r_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, String> {
    Ok(match r.u8()? {
        0 => Value::Nil,
        1 => Value::Bool(false),
        2 => Value::Bool(true),
        3 => Value::Int(i64::from_le_bytes(r.take(8)?.try_into().unwrap())),
        4 => Value::Float(f64::from_bits(u64::from_le_bytes(
            r.take(8)?.try_into().unwrap(),
        ))),
        5 => {
            let b = r.bytes()?;
            Value::Str(heap.intern(b))
        }
        t => return Err(format!("bad constant tag {t}")),
    })
}

fn r_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::LuaStr>>,
) -> Result<Gc<Proto>, String> {
    let num_params = r.u8()?;
    let is_vararg = r.u8()? != 0;
    let max_stack = r.u8()?;
    let line_defined = r.u32()?;
    let last_line_defined = r.u32()?;
    // PUC `LoadFunction`: an empty source means "inherit parent's", because
    // the dumper writes nothing when this proto shares the parent's source.
    let raw = r.bytes()?;
    let source = if raw.is_empty() {
        parent_source.unwrap_or_else(|| heap.intern(b""))
    } else {
        heap.intern(raw)
    };

    let n = r.u32()? as usize;
    let mut code = Vec::with_capacity(n);
    for _ in 0..n {
        code.push(crate::vm::isa::Inst(r.u32()?));
    }
    let n = r.u32()? as usize;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        lines.push(r.u32()?);
    }
    let n = r.u32()? as usize;
    let mut consts = Vec::with_capacity(n);
    for _ in 0..n {
        consts.push(r_const(r, heap)?);
    }
    let n = r.u32()? as usize;
    let mut upvals = Vec::with_capacity(n);
    for _ in 0..n {
        let in_stack = r.u8()? != 0;
        let index = r.u8()?;
        let read_only = r.u8()? != 0;
        let name = String::from_utf8_lossy(r.bytes()?).into_owned().into();
        upvals.push(UpvalDesc {
            in_stack,
            index,
            name,
            read_only,
        });
    }
    let n = r.u32()? as usize;
    let mut protos = Vec::with_capacity(n);
    for _ in 0..n {
        protos.push(r_proto(r, heap, Some(source))?);
    }
    let n = r.u32()? as usize;
    let mut locvars = Vec::with_capacity(n);
    for _ in 0..n {
        let name = String::from_utf8_lossy(r.bytes()?).into_owned().into();
        let reg = r.u32()?;
        let start_pc = r.u32()?;
        let end_pc = r.u32()?;
        locvars.push(LocVar {
            name,
            reg,
            start_pc,
            end_pc,
        });
    }

    // PUC binary chunks do not carry the per-proto `has_vararg_table_pseudo`
    // bit (it's an implementation detail of the source-level parlist), so a
    // loaded vararg proto conservatively reports no pseudo — `(vararg table)`
    // would be returned by `lua_getlocal` only on protos compiled here.
    let env_upval_idx = upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);
    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: code.into_boxed_slice(),
        consts: consts.into_boxed_slice(),
        protos: protos.into_boxed_slice(),
        upvals: upvals.into_boxed_slice(),
        num_params,
        is_vararg,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack,
        lines: lines.into_boxed_slice(),
        source,
        line_defined,
        last_line_defined,
        locvars: locvars.into_boxed_slice(),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        traces: std::cell::RefCell::new(Vec::new()),
    }))
}

/// Reconstruct a prototype tree from a binary chunk produced by [`dump`].
/// Validates the running dialect's PUC header byte-for-byte (the calls.lua
/// corrupted-header test flips a single byte and expects a load failure),
/// then the luna body tag, then the luna body.
pub fn undump(bytes: &[u8], heap: &mut Heap, version: LuaVersion) -> Result<Gc<Proto>, String> {
    let header = header_for(version);
    if bytes.len() < header.len() {
        return Err("truncated binary chunk".to_string());
    }
    // Validate everything except the trailing float sanity field (PUC tolerates
    // long-double padding differences here, and on this build the float
    // representation matches anyway). The non-float bytes are luna's
    // contract: a single-byte change must fail the load.
    let float_off = header.len() - 8;
    if bytes[..float_off] != header[..float_off] {
        return Err("bad binary chunk header".to_string());
    }
    if bytes[float_off..header.len()] != header[float_off..] {
        return Err("bad binary chunk float check".to_string());
    }
    let pos = header.len();
    if bytes.len() < pos + BODY_TAG.len() {
        return Err("truncated binary chunk".to_string());
    }
    if &bytes[pos..pos + BODY_TAG.len()] != BODY_TAG {
        return Err("bad binary chunk body tag".to_string());
    }
    let mut r = Reader {
        b: bytes,
        p: pos + BODY_TAG.len(),
    };
    let proto = r_proto(&mut r, heap, None)?;
    if r.p != bytes.len() {
        return Err("trailing bytes in chunk".to_string());
    }
    Ok(proto)
}
