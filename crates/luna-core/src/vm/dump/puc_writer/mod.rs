//! luna `Proto` → PUC bytecode, the inverse of [`super::puc`].
//!
//! `string.dump` writes the running dialect's own PUC chunk format, so the
//! stock interpreter of that version loads what luna dumps. Each function
//! is re-encoded instruction by instruction ([`modern`] for 5.4/5.5,
//! [`classic`] for 5.1–5.3, on the shared [`asm`] state); jumps, line info
//! and local-variable ranges follow the code through the pc map, and the
//! result is serialised as that version's `ldump.c` would ([`format`]).
//!
//! PUC dumps no register for a local: `getlocalname` takes the n-th local
//! active at a pc to live in register n-1. luna's frame follows PUC's
//! register discipline, and where its layout differs (the hidden slots of
//! `for` loops, 5.5's vararg parameter) the encoders renumber registers so
//! that the rule holds for the written code.
//!
//! A function luna cannot express in the dialect's instruction set is
//! refused (`Err`) rather than approximated.

mod asm;
mod classic;
mod classic_const;
mod classic_flow;
mod classic_ops;
mod format;
mod modern;
mod modern_const;
mod modern_flow;
mod proto_parts;

use self::asm::{Asm, Res, loop_windows};
use self::proto_parts::{check_skips, consts_for, needs_close, vararg_byte};
use super::puc::{puc_52, puc_53, puc_54, puc_55};
use crate::compiler::const_map::const_map_of;
use crate::runtime::Value;
use crate::runtime::function::Proto;
use crate::version::LuaVersion;
use crate::vm::isa::Op;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Dialect {
    V51,
    V52,
    V53,
    V54,
    V55,
}

impl Dialect {
    fn name(self) -> &'static str {
        match self {
            Dialect::V51 => "PUC 5.1",
            Dialect::V52 => "PUC 5.2",
            Dialect::V53 => "PUC 5.3",
            Dialect::V54 => "PUC 5.4",
            Dialect::V55 => "PUC 5.5",
        }
    }

    fn version(self) -> LuaVersion {
        match self {
            Dialect::V51 => LuaVersion::Lua51,
            Dialect::V52 => LuaVersion::Lua52,
            Dialect::V53 => LuaVersion::Lua53,
            Dialect::V54 => LuaVersion::Lua54,
            Dialect::V55 => LuaVersion::Lua55,
        }
    }
}

struct Up {
    in_stack: bool,
    index: u8,
    /// 5.4/5.5 `kind`: 1 (`<const>`) for a read-only capture, else 0
    kind: u8,
    name: crate::runtime::DebugName,
}

/// One function, re-encoded, ready to serialise.
struct Out {
    source: Vec<u8>,
    line_defined: u32,
    last_line_defined: u32,
    num_params: u8,
    /// the dialect's `is_vararg` / `flag` byte
    vararg: u8,
    max_stack: u8,
    code: Vec<u32>,
    /// absolute line per instruction; empty when there is none
    lines: Vec<u32>,
    consts: Vec<Value>,
    upvals: Vec<Up>,
    protos: Vec<Out>,
    locvars: Vec<(crate::runtime::DebugName, u32, u32)>,
}

/// Serialise `proto` as a chunk of the PUC version `version` names;
/// `strip` drops debug information as PUC's `strip` does.
pub(crate) fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Result<Vec<u8>, String> {
    dump_blocks(proto, strip, version).map(|(bytes, _)| bytes)
}

/// [`dump`], with the size of each block PUC's dumper writes separately.
pub(crate) fn dump_blocks(
    proto: &Proto,
    strip: bool,
    version: LuaVersion,
) -> Result<(Vec<u8>, Vec<usize>), String> {
    let d = match version {
        LuaVersion::Lua51 => Dialect::V51,
        LuaVersion::Lua52 => Dialect::V52,
        LuaVersion::Lua53 => Dialect::V53,
        LuaVersion::Lua54 => Dialect::V54,
        LuaVersion::Lua55 => Dialect::V55,
        LuaVersion::MacroLua => return Err("MacroLua has no PUC bytecode format".to_string()),
    };
    let root = build(proto, d, None)?;
    Ok(format::write(d, &root, strip))
}

/// `caps`: the upvalue descriptors the parent's `Closure` site gave this
/// function after renumbering its registers (`None` for the main function).
fn build(p: &Proto, d: Dialect, caps: Option<Vec<(bool, u8)>>) -> Res<Out> {
    let np = p.num_params as u32;
    let windows = match d {
        Dialect::V54 => Vec::new(),
        Dialect::V55 => loop_windows(p, 3, Some(3))?,
        _ => loop_windows(p, 4, None)?,
    };
    let mut frame = p.max_stack as u32;
    if d == Dialect::V55 && p.is_vararg {
        frame = frame.max(np + 1);
    }
    let mut asm = Asm::new(p, d.name(), windows, frame);
    // 5.1 / 5.2 have one number type: the constants the encoder adds meet
    // luna's as the floats these become
    if d <= Dialect::V52 {
        asm.consts = consts_for(d, std::mem::take(&mut asm.consts))?;
    }
    let ver = d.version();
    asm.kmap = (ver, const_map_of(ver, &asm.consts));
    let mut child_caps: modern::Caps = vec![None; p.protos.len()];
    let vatab = d == Dialect::V55
        && p.is_vararg
        && p.code
            .first()
            .is_some_and(|i| i.op() == Op::GetVarg && i.a() == np);
    if d >= Dialect::V54 {
        let v55 = d == Dialect::V55;
        let hidden = p.is_vararg && !(v55 && vatab);
        let f = modern::Frame {
            ops: if v55 { puc_55::OPS } else { puc_54::OPS },
            v55,
            np,
            ret_c: if hidden { np + 1 } else { 0 },
            needclose: needs_close(p),
            vatab,
        };
        modern::encode(&mut asm, &f, vatab, &mut child_caps)?;
    } else {
        let f = classic::Frame {
            ver: match d {
                Dialect::V51 => 51,
                Dialect::V52 => 52,
                _ => 53,
            },
            ops: if d == Dialect::V52 {
                puc_52::OPS
            } else {
                puc_53::OPS
            },
        };
        classic::encode(&mut asm, &f, &mut child_caps)?;
    }
    let loc_regs: Vec<u32> = p
        .locvars
        .iter()
        .map(|v| asm.reg_at(v.start_pc as usize, v.reg).unwrap_or(v.reg))
        .collect();
    let body = asm.finish()?;
    check_skips(p, &body.pc_map)?;
    let limit = if d == Dialect::V51 { 250 } else { 255 };
    if body.frame > limit {
        return Err(format!(
            "{}: function needs {} registers",
            d.name(),
            body.frame
        ));
    }

    let mut locvars: Vec<(u32, u32, (crate::runtime::DebugName, u32, u32))> = p
        .locvars
        .iter()
        .zip(&loc_regs)
        .map(|(v, &reg)| {
            let start = if v.start_pc == 0 && v.reg < np {
                0
            } else {
                body.pc_map[(v.start_pc as usize).min(p.code.len())]
            };
            let end = body.pc_map[(v.end_pc as usize).min(p.code.len())];
            (start, reg, (v.name.clone(), start, end))
        })
        .collect();
    locvars.sort_by_key(|v| (v.0, v.1));

    // PUC's `mainfunc` describes a main chunk's `_ENV` as register 0 of the
    // (absent) enclosing function: in the stack, index 0
    let main = caps.is_none() && p.line_defined == 0;
    let own: Vec<(bool, u8)> = p
        .upvals
        .iter()
        .enumerate()
        .map(|(k, u)| match k {
            0 if main && &*u.name == "_ENV" => (true, 0),
            _ => (u.in_stack, u.index),
        })
        .collect();
    let caps = caps.unwrap_or(own);
    let skip_env = usize::from(d == Dialect::V51);
    let upvals = p
        .upvals
        .iter()
        .zip(caps)
        .skip(skip_env)
        .map(|(u, (in_stack, index))| Up {
            in_stack,
            index,
            kind: u.read_only as u8,
            name: u.name.clone(),
        })
        .collect();

    let mut protos = Vec::with_capacity(p.protos.len());
    for (child, cap) in p.protos.iter().zip(child_caps) {
        protos.push(build(child, d, cap)?);
    }
    Ok(Out {
        source: p.source.as_bytes().to_vec(),
        line_defined: p.line_defined,
        last_line_defined: p.last_line_defined,
        num_params: p.num_params,
        vararg: vararg_byte(p, d, vatab),
        max_stack: body.frame as u8,
        code: body.code,
        lines: body.lines,
        consts: consts_for(d, body.consts)?,
        upvals,
        protos,
        locvars: locvars.into_iter().map(|v| v.2).collect(),
    })
}
