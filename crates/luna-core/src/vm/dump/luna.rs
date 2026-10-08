//! luna's own `string.dump` / chunk undump (per-dialect PUC header +
//! luna-specific body).
//!
//! The header mirrors PUC's per-version layout (calls.lua's `headformat`
//! round-trips with the matching component values for whichever dialect is
//! running), so a corrupted-header test rejects luna's chunks the same way
//! PUC would. The body that follows is luna-specific — luna's VM cannot
//! execute PUC bytecode through this path (different opcode encoding,
//! register conventions, etc.); PUC bytecode loading lives in
//! `super::puc`. `strip` drops debug names (local-variable
//! records and upvalue names); line info is always kept because the VM
//! indexes it for error positions.

use super::error::Bad;
use super::header;
use super::reader::Reader;
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
    0x1b, b'L', b'u', b'a', 0x55, 0x00, 0x19, 0x93, b'\r', b'\n', 0x1a, b'\n', 4, 0x88, 0xa9, 0xff,
    0xff, 4, 0x78, 0x56, 0x34, 0x12, 8, 0x88, 0xa9, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 8, 0, 0, 0,
    0, 0, 0x28, 0x77, 0xc0,
];

/// PUC 5.4 binary-chunk header (31 bytes), per `ldump.c DumpHeader`:
/// signature + 0x54 + format + LUAC_DATA + sizeof(Instruction) +
/// sizeof(lua_Integer) + sizeof(lua_Number) + LUAC_INT (0x5678) +
/// LUAC_NUM (370.5). calls.lua :395 packs the first 15 bytes plus an
/// `(jn)` unpack of the next 16 to lock these values in.
const HEADER_54: &[u8] = &[
    0x1b, b'L', b'u', b'a', 0x54, 0x00, 0x19, 0x93, b'\r', b'\n', 0x1a, b'\n',
    4, // sizeof(Instruction)
    8, // sizeof(lua_Integer)
    8, // sizeof(lua_Number)
    0x78, 0x56, 0, 0, 0, 0, 0, 0, // LUAC_INT = 0x5678
    0, 0, 0, 0, 0, 0x28, 0x77, 0x40, // LUAC_NUM = 370.5
];

pub(super) fn header_for(version: LuaVersion) -> &'static [u8] {
    header_and_layout(version).0
}

/// The header luna writes for `version`, with its PUC field layout.
fn header_and_layout(version: LuaVersion) -> (&'static [u8], &'static [(usize, Bad)]) {
    match version {
        LuaVersion::Lua53 => (header::HEADER_53, header::LAYOUT_53),
        LuaVersion::Lua54 => (HEADER_54, header::LAYOUT_54),
        // 5.1 / 5.2 calls.lua does not test binary-chunk header bytes, so
        // route them through the 5.5 layout (luna's own dump round-trips
        // either way, and PUC 5.1/5.2 chunks aren't loadable into luna).
        LuaVersion::Lua51 | LuaVersion::Lua52 | LuaVersion::Lua55 | LuaVersion::MacroLua => {
            (HEADER_55, header::LAYOUT_55)
        }
    }
}

/// luna's body-format tag, written immediately after the PUC header. PUC's
/// loader would reach this byte expecting the number of upvalues; we use a
/// non-PUC sentinel so an accidental cross-load (luna chunk into PUC, or
/// vice-versa) errors cleanly rather than misinterpreting bytes.
pub(super) const BODY_TAG: &[u8] = b"\x00LunaV4\x00";

mod write;
pub(super) use write::dump;

// `Reader` lives in `super::reader` so the per-dialect PUC translators
// (`super::puc`) can share the same primitives.

fn r_const(
    r: &mut Reader,
    heap: &mut Heap,
    strings: &mut Vec<Gc<crate::runtime::LuaStr>>,
) -> Result<Value, Bad> {
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
            let s = heap.intern(b);
            strings.push(s);
            Value::Str(s)
        }
        // a string saved earlier in the chunk (5.5)
        6 => {
            let idx = r.u32()? as usize;
            Value::Str(
                *strings
                    .get(idx)
                    .ok_or(Bad::Code(format!("saved string {idx} out of range")))?,
            )
        }
        _ => return Err(Bad::Constant),
    })
}

/// A u32 element count, refused as a truncation when the rest of the
/// chunk cannot hold that many elements of at least `min_size` bytes.
fn read_count(r: &mut Reader, min_size: usize) -> Result<usize, Bad> {
    let n = r.u32()?;
    r.count(u64::from(n), min_size)
}

fn r_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::LuaStr>>,
    strings: &mut Vec<Gc<crate::runtime::LuaStr>>,
) -> Result<Gc<Proto>, Bad> {
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

    // each count sizes an allocation; `count` refuses one the remaining
    // bytes cannot hold (the minimum size of an element) as a truncation
    let n = read_count(r, 4)?;
    let mut code = Vec::with_capacity(n);
    for _ in 0..n {
        code.push(crate::vm::isa::Inst(r.u32()?));
    }
    let n = read_count(r, 4)?;
    let mut lines = Vec::with_capacity(n);
    for _ in 0..n {
        lines.push(r.u32()?);
    }
    let n = read_count(r, 1)?;
    let mut consts = Vec::with_capacity(n);
    for _ in 0..n {
        consts.push(r_const(r, heap, strings)?);
    }
    let n = read_count(r, 7)?;
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
    let n = read_count(r, 4)?;
    let mut protos = Vec::with_capacity(n);
    for _ in 0..n {
        protos.push(r.nested(|r| r_proto(r, heap, Some(source), strings))?);
    }
    let n = read_count(r, 16)?;
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
    crate::runtime::function_close::mark_closing_returns(&mut code, &protos);
    let env_upval_idx = upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);
    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: heap.block_of(code.into_iter()),
        consts: heap.block_of(consts.into_iter()),
        protos: heap.block_of(protos.into_iter()),
        upvals: heap.block_of(upvals.into_iter()),
        num_params,
        is_vararg,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack,
        lines: heap.block_of(lines.into_iter()),
        source,
        line_defined,
        last_line_defined,
        locvars: heap.block_of(locvars.into_iter()),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
        inlined_protos: std::cell::RefCell::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        has_dispatchable_trace: std::cell::Cell::new(false),
        trace_heads: std::cell::Cell::new(
            [crate::runtime::function::TRACE_HEADS_NONE; crate::runtime::function::TRACE_HEADS_CAP],
        ),
        trace_call_head_settled: std::cell::Cell::new(false),
    }))
}

/// Reconstruct a prototype tree from a binary chunk produced by [`dump`].
/// Validates the running dialect's PUC header byte-for-byte (the calls.lua
/// corrupted-header test flips a single byte and expects a load failure),
/// then the luna body tag, then the luna body.
pub(super) fn undump(bytes: &[u8], heap: &mut Heap, version: LuaVersion) -> Result<Gc<Proto>, Bad> {
    let (header, layout) = header_and_layout(version);
    header::check(bytes, header, layout)?;
    let body = header.len();
    let tag = bytes
        .get(body..body + BODY_TAG.len())
        .ok_or(Bad::Truncated)?;
    if tag != BODY_TAG {
        return Err(Bad::Code("not a luna chunk body".to_string()));
    }
    let mut r = Reader::at(bytes, body + BODY_TAG.len());
    r_proto(&mut r, heap, None, &mut Vec::new())
}
