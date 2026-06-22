//! Recover a source-level name for a register, for variable-aware error
//! messages (PUC ldebug.c getobjname/findsetreg). Given the bytecode and the
//! pc of the faulting instruction, trace a register back to the instruction
//! that last wrote it and classify it as a field/global/upvalue/method/etc.
//!
//! Local-variable names are not yet recovered (the Proto carries no locvar
//! debug table), so a register sourced from a plain local yields no name;
//! callers then emit a bare message without the "(kind 'name')" suffix.

use crate::runtime::Value;
use crate::runtime::function::Proto;
use crate::vm::isa::{Inst, Op};

/// The string constant at index `c`, if it is a string.
fn kname(proto: &Proto, c: u32) -> Option<String> {
    match proto.consts.get(c as usize) {
        Some(Value::Str(s)) => Some(String::from_utf8_lossy(s.as_bytes()).into_owned()),
        _ => None,
    }
}

fn upvalname(proto: &Proto, u: u32) -> Option<String> {
    proto.upvals.get(u as usize).map(|d| d.name.to_string())
}

/// Does instruction `i` write to register `reg`?
fn writes_reg(i: Inst, reg: u32) -> bool {
    let a = i.a();
    match i.op() {
        Op::LoadNil => a <= reg && reg <= a + i.b(),
        Op::SelfOp => reg == a || reg == a + 1,
        // these write a run of registers starting at A (results / varargs)
        Op::Call | Op::TailCall | Op::Vararg => reg >= a,
        Op::TForCall => reg >= a + 4,
        // control / store / no-result opcodes write no destination register
        Op::Jmp
        | Op::SetUpval
        | Op::SetTabUp
        | Op::SetTable
        | Op::SetI
        | Op::SetField
        | Op::Close
        | Op::Tbc
        | Op::Eq
        | Op::Lt
        | Op::Le
        | Op::EqK
        | Op::Test
        | Op::Return
        | Op::Return0
        | Op::Return1
        | Op::SetList
        | Op::ExtraArg
        | Op::TForPrep => false,
        _ => reg == a,
    }
}

/// PUC findsetreg: the pc of the last instruction before `lastpc` that sets
/// `reg`, or None if it is set inside a jump target region (ambiguous) or
/// never set.
fn find_setreg(proto: &Proto, lastpc: usize, reg: u32) -> Option<usize> {
    let mut setreg: Option<usize> = None;
    let mut jmptarget: usize = 0;
    let code = &proto.code;
    for pc in 0..lastpc {
        let i = code[pc];
        if i.op() == Op::Jmp {
            let dest = (pc as i64 + 1 + i.sj() as i64) as usize;
            if dest <= lastpc && dest > jmptarget {
                jmptarget = dest;
            }
            continue;
        }
        if writes_reg(i, reg) {
            setreg = if pc < jmptarget { None } else { Some(pc) };
        }
    }
    setreg
}

/// PUC basicgetobjname: a register's name reached only through locals, plain
/// register moves, and upvalues — never through table indexing. `gxf` needs
/// this so a field that merely happens to be named `_ENV` (e.g. `a._ENV.x`) is
/// not mistaken for the real global environment.
fn basicgetobjname(proto: &Proto, lastpc: usize, reg: u32) -> Option<(&'static str, String)> {
    if let Some(name) = getlocalname(proto, reg, lastpc) {
        return Some(("local", name.to_string()));
    }
    let setpc = find_setreg(proto, lastpc, reg)?;
    let i = proto.code[setpc];
    match i.op() {
        Op::Move => {
            let b = i.b();
            if b < i.a() {
                basicgetobjname(proto, setpc, b)
            } else {
                None
            }
        }
        Op::GetUpval => upvalname(proto, i.b()).map(|n| ("upvalue", n)),
        Op::LoadK => kname(proto, i.bx()).map(|n| ("constant", n)),
        // LoadKx is the 32-bit-index variant: the constant index lives in the
        // immediately-following ExtraArg op's Ax field, so a name can still be
        // recovered (5.4/5.3 big.lua's huge prog blows past LoadK's 18-bit
        // limit and would otherwise erase the "(global 'X')" subject info).
        Op::LoadKx => {
            let next = proto.code.get(setpc + 1)?;
            if next.op() != Op::ExtraArg {
                return None;
            }
            kname(proto, next.ax()).map(|n| ("constant", n))
        }
        _ => None,
    }
}

/// PUC rname: the name of register `c` when it holds a constant key (e.g. a
/// global read compiled as `GETTABLE` with the key LOADK'd into a register);
/// "?" otherwise.
fn rname(proto: &Proto, pc: usize, c: u32) -> String {
    match basicgetobjname(proto, pc, c) {
        Some(("constant", n)) => n,
        _ => "?".to_string(),
    }
}

/// gxf (PUC isEnv): an indexed access names a "global" when the table is the
/// real `_ENV`, else a "field". `isup` distinguishes GETTABUP (table is an
/// upvalue) from GETFIELD. The table only counts as `_ENV` when it is reached
/// as a local or an upvalue — not as some other field named `_ENV`.
fn gxf(proto: &Proto, pc: usize, i: Inst, isup: bool) -> &'static str {
    let t = i.b();
    let tname = if isup {
        upvalname(proto, t)
    } else {
        match basicgetobjname(proto, pc, t) {
            Some((kind, n)) if kind == "local" || kind == "upvalue" => Some(n),
            _ => None,
        }
    };
    if tname.as_deref() == Some("_ENV") {
        "global"
    } else {
        "field"
    }
}

/// The name of the local variable occupying `reg` and live at `pc`, if any.
/// On register reuse the innermost (latest-starting) live local wins.
pub fn getlocalname(proto: &Proto, reg: u32, pc: usize) -> Option<&str> {
    let pc = pc as u32;
    proto
        .locvars
        .iter()
        .filter(|lv| lv.reg == reg && lv.start_pc <= pc && pc < lv.end_pc)
        .max_by_key(|lv| lv.start_pc)
        .map(|lv| &*lv.name)
}

/// Name and kind for `reg` as of `lastpc`, e.g. ("field", "huge"). None when
/// the register has no recoverable source-level name.
pub fn getobjname(proto: &Proto, lastpc: usize, reg: u32) -> Option<(&'static str, String)> {
    // PUC order: a live local takes precedence over symbolic execution
    if let Some(name) = getlocalname(proto, reg, lastpc) {
        return Some(("local", name.to_string()));
    }
    let setpc = find_setreg(proto, lastpc, reg)?;
    let i = proto.code[setpc];
    match i.op() {
        Op::Move => {
            let b = i.b();
            // trace the source register, but only backwards to avoid cycles
            if b < i.a() {
                getobjname(proto, setpc, b)
            } else {
                None
            }
        }
        Op::GetUpval => upvalname(proto, i.b()).map(|n| ("upvalue", n)),
        Op::GetTabUp => kname(proto, i.c()).map(|n| (gxf(proto, setpc, i, true), n)),
        Op::GetField => kname(proto, i.c()).map(|n| (gxf(proto, setpc, i, false), n)),
        // a register-keyed read (global with a constant index past the GETFIELD
        // C-operand limit, or an explicit `t[k]`): name from the key register.
        Op::GetTable => Some((gxf(proto, setpc, i, false), rname(proto, setpc, i.c()))),
        Op::GetI => Some(("field", "integer index".to_string())),
        Op::SelfOp => {
            // SELF's C is RK: a constant when the k-flag is set, otherwise a
            // register holding the (constant-loaded) key. The latter path is
            // taken when the method-name constant index doesn't fit OP_SELF's
            // 8-bit C field (errors.lua :303 — `t:bbb()` past 1000 constants).
            let name = if i.k() {
                kname(proto, i.c())
            } else {
                Some(rname(proto, setpc, i.c()))
            };
            name.map(|n| ("method", n))
        }
        _ => None,
    }
}
