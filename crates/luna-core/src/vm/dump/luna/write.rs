//! Writing luna's own body format.

use super::*;

fn w_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn w_bytes(out: &mut Vec<u8>, b: &[u8]) {
    w_u32(out, b.len() as u32);
    out.extend_from_slice(b);
}

/// Strings already written, for 5.5's `dumpString`, which saves each
/// distinct string once and refers back to it after that; `None` for the
/// dialects whose dump repeats them.
type Saved = Option<std::collections::HashMap<Vec<u8>, u32>>;

fn w_const(out: &mut Vec<u8>, v: Value, saved: &mut Saved) {
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
            if let Some(map) = saved {
                if let Some(&idx) = map.get(s.as_bytes()) {
                    out.push(6);
                    w_u32(out, idx);
                    return;
                }
                let idx = map.len() as u32;
                map.insert(s.as_bytes().to_vec(), idx);
            }
            out.push(5);
            w_bytes(out, s.as_bytes());
        }
        // A constant table can only hold the above (the compiler never emits
        // table/function constants); anything else is a bug.
        other => unreachable!("non-serialisable constant: {}", other.type_name()),
    }
}

fn w_proto(
    out: &mut Vec<u8>,
    p: &Proto,
    strip: bool,
    parent_source: Option<&[u8]>,
    saved: &mut Saved,
) {
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
        w_const(out, k, saved);
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
        w_proto(out, sub, strip, Some(source), saved);
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
pub(in crate::vm::dump) fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Vec<u8> {
    let header = header_for(version);
    let mut out = Vec::with_capacity(header.len() + BODY_TAG.len() + proto.code.len() * 4);
    out.extend_from_slice(header);
    out.extend_from_slice(BODY_TAG);
    let mut saved: Saved = (version >= LuaVersion::Lua55).then(Default::default);
    w_proto(&mut out, proto, strip, None, &mut saved);
    out
}
