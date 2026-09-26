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
mod classic_flow;
mod format;
mod modern;
mod modern_flow;

use self::asm::{Asm, Res, Window, loop_windows};
use super::puc::{puc_52, puc_53, puc_54, puc_55};
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
}

struct Up {
    in_stack: bool,
    index: u8,
    /// 5.4/5.5 `kind`: 1 (`<const>`) for a read-only capture, else 0
    kind: u8,
    name: Box<str>,
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
    locvars: Vec<(Box<str>, u32, u32)>,
}

/// Serialise `proto` as a chunk of the PUC version `version` names;
/// `strip` drops debug information as PUC's `strip` does.
pub(crate) fn dump(proto: &Proto, strip: bool, version: LuaVersion) -> Result<Vec<u8>, String> {
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
    let mut windows = match d {
        Dialect::V54 => Vec::new(),
        Dialect::V55 => loop_windows(p, 3, Some(3))?,
        _ => loop_windows(p, 4, None)?,
    };
    // 5.5 gives an anonymous `...` parameter a register after the fixed
    // ones; luna has none, so every register from there moves up one
    let vararg_slot = d == Dialect::V55 && p.has_vararg_table_pseudo;
    if vararg_slot {
        windows.push(Window {
            first: 0,
            last: p.code.len().saturating_sub(1),
            pivot: np,
            delta: 1,
        });
    }
    let mut frame = p.max_stack as u32 + vararg_slot as u32;
    if d == Dialect::V55 && p.is_vararg {
        frame = frame.max(np + 1);
    }
    let mut asm = Asm::new(p, d.name(), windows, frame);
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

    let mut locvars: Vec<(u32, u32, (Box<str>, u32, u32))> = p
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
    if vararg_slot {
        let n = body.code.len() as u32;
        locvars.push((1, np, ("(vararg table)".into(), 1, n)));
    }
    locvars.sort_by_key(|v| (v.0, v.1));

    let own: Vec<(bool, u8)> = p.upvals.iter().map(|u| (u.in_stack, u.index)).collect();
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

/// PUC's `needclose`: some local is captured by a closure or is to be
/// closed, so returns and tail calls must close upvalues first.
fn needs_close(p: &Proto) -> bool {
    p.code.iter().any(|i| matches!(i.op(), Op::Close | Op::Tbc))
        || p.protos.iter().any(|c| c.upvals.iter().any(|u| u.in_stack))
}

fn vararg_byte(p: &Proto, d: Dialect, vatab: bool) -> u8 {
    if !p.is_vararg {
        return 0;
    }
    match d {
        // VARARG_ISVARARG, plus VARARG_HASARG | VARARG_NEEDSARG for the
        // implicit `arg` table
        Dialect::V51 if p.has_compat_vararg_arg => 7,
        Dialect::V51 => 2,
        // PF_VATAB or PF_VAHID
        Dialect::V55 if vatab => 2,
        _ => 1,
    }
}

/// 5.1 and 5.2 have one number type: luna's integers become floats.
fn consts_for(d: Dialect, consts: Vec<Value>) -> Res<Vec<Value>> {
    if d > Dialect::V52 {
        return Ok(consts);
    }
    consts
        .into_iter()
        .map(|v| match v {
            Value::Int(i) if (i as f64) as i64 == i && i != i64::MAX => Ok(Value::Float(i as f64)),
            Value::Int(i) => Err(format!("{}: integer constant {i} has no float", d.name())),
            v => Ok(v),
        })
        .collect()
}

/// An instruction that skips the next one on some path skips exactly one
/// PUC instruction, so what follows it must still be a single instruction.
fn check_skips(p: &Proto, pc_map: &[u32]) -> Res<()> {
    for (pc, i) in p.code.iter().enumerate() {
        let skips = matches!(
            i.op(),
            Op::LFalseSkip | Op::Eq | Op::Lt | Op::Le | Op::EqK | Op::Test | Op::TestSet
        );
        if skips && (pc + 2 >= pc_map.len() || pc_map[pc + 2] - pc_map[pc + 1] != 1) {
            return Err(format!(
                "instruction {} skips one that has no single-instruction form",
                pc + 1
            ));
        }
    }
    Ok(())
}
