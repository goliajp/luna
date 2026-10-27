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
//! active at a pc to live in register n-1. luna's compiler gives every
//! value the register PUC's gives it, so the encoders write registers as
//! they are, and the locals in the order of the pcs they start at.
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
mod width;

pub(crate) use self::width::Layout;
#[cfg(any(debug_assertions, feature = "gc-trace"))]
pub(crate) use self::width::check_compiled;

use self::asm::{Asm, Res};
use self::proto_parts::{check_skips, consts_for, needs_close, vararg_byte};

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

    fn of(version: LuaVersion) -> Option<Dialect> {
        Some(match version {
            LuaVersion::Lua51 => Dialect::V51,
            LuaVersion::Lua52 => Dialect::V52,
            LuaVersion::Lua53 => Dialect::V53,
            LuaVersion::Lua54 => Dialect::V54,
            LuaVersion::Lua55 => Dialect::V55,
            LuaVersion::MacroLua => return None,
        })
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
#[derive(Default)]
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

/// A function as the encoder lays it out (see [`Layout`]).
pub(crate) struct Shape {
    /// luna pc → the first PUC instruction for it; one past the end
    pub pc_map: Vec<u32>,
    /// PUC's line of each PUC instruction
    pub lines: Vec<u32>,
    /// the luna pc at which each constant PUC has and luna lacks is added
    pub k_added: Vec<usize>,
}

/// The [`Shape`] of `p` alone.
fn shape_one(p: &Proto, d: Dialect) -> Res<Shape> {
    let mut caps: modern::Caps = vec![None; p.protos.len()];
    let body = encode_body(p, d, true, &mut caps)?;
    Ok(Shape {
        pc_map: body.pc_map,
        lines: body.lines,
        k_added: body.k_added,
    })
}

/// The PUC chunk of `proto`, the size of each block PUC's dumper writes
/// separately, and how many strings 5.5's dumper saved.
pub(crate) fn dump_blocks(
    proto: &Proto,
    strip: bool,
    version: LuaVersion,
) -> Result<(Vec<u8>, Vec<usize>, usize), String> {
    let d = Dialect::of(version).ok_or("MacroLua has no PUC bytecode format")?;
    let root = build(proto, d, None)?;
    Ok(format::write(d, &root, strip))
}

/// `caps`: the upvalue descriptors the parent's `Closure` site gave this
/// function (`None` for the main function).
fn build(p: &Proto, d: Dialect, caps: Option<Vec<(bool, u8)>>) -> Res<Out> {
    let np = p.num_params as u32;
    let mut child_caps: modern::Caps = vec![None; p.protos.len()];
    let body = encode_body(p, d, false, &mut child_caps)?;
    let vatab = width::vatab(p, d);

    let mut locvars: Vec<(u32, u32, (crate::runtime::DebugName, u32, u32))> = p
        .locvars
        .iter()
        .map(|v| {
            let reg = v.reg;
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
        vararg: vararg_byte(p, d, vatab, main),
        max_stack: body.frame as u8,
        code: body.code,
        lines: body.lines,
        consts: consts_for(d, body.consts)?,
        upvals,
        protos,
        locvars: locvars.into_iter().map(|v| v.2).collect(),
    })
}

/// `p`'s code assembled for dialect `d` (`shape_only`: the instruction
/// words left out), with the upvalue descriptors of its children.
fn encode_body(
    p: &Proto,
    d: Dialect,
    shape_only: bool,
    child_caps: &mut modern::Caps,
) -> Res<asm::Body> {
    let np = p.num_params as u32;
    let mut asm = Asm::new(p, d.name());
    asm.shape_only = shape_only;
    // 5.1 / 5.2 have one number type: the constants the encoder adds meet
    // luna's as the floats these become
    if d <= Dialect::V52 {
        asm.consts = consts_for(d, std::mem::take(&mut asm.consts))?;
    }
    let ver = d.version();
    asm.kmap = (ver, None);
    let vatab = width::vatab(p, d);
    if d >= Dialect::V54 {
        let v55 = d == Dialect::V55;
        let hidden = p.is_vararg && !(v55 && vatab);
        let f = modern::Frame {
            ops: if v55 {
                &crate::vm::dump::puc::opcodes::OPCODES_55
            } else {
                &crate::vm::dump::puc::opcodes::OPCODES_54
            },
            v55,
            np,
            ret_c: if hidden { np + 1 } else { 0 },
            needclose: needs_close(p),
            vatab,
        };
        modern::encode(&mut asm, &f, vatab, child_caps)?;
    } else {
        let f = classic::Frame {
            ver: match d {
                Dialect::V51 => 51,
                Dialect::V52 => 52,
                _ => 53,
            },
            ops: if d == Dialect::V52 {
                &crate::vm::dump::puc::opcodes::OPCODES_52
            } else {
                &crate::vm::dump::puc::opcodes::OPCODES_53
            },
        };
        classic::encode(&mut asm, &f, child_caps)?;
    }
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
    #[cfg(any(debug_assertions, feature = "gc-trace"))]
    width::check(p, d, &body);
    Ok(body)
}
