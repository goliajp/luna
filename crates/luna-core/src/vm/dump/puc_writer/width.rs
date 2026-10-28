//! How many PUC instructions the encoders write for each luna instruction,
//! without encoding: what the load's accounting needs of PUC's layout (see
//! `vm/exec/load_acct.rs`).
//!
//! The rules the encoders branch on are here and the encoders call them;
//! the counts that follow from a branch are written once more below, and
//! `build` checks them against what the encoders emitted in debug and
//! gc-trace builds.
//!
//! An `RK` operand or an immediate's constant past index 255 costs an extra
//! `LOADK`, and finding an immediate's constant index takes a search; a
//! function with that many constants is encoded instead.

use super::Dialect;
use crate::runtime::Value;
use crate::runtime::function::Proto;
use crate::version::LuaVersion;
use crate::vm::isa::{MAX_SC, MIN_SC, Op};

#[cfg(any(debug_assertions, feature = "gc-trace"))]
mod check;
#[cfg(any(debug_assertions, feature = "gc-trace"))]
pub(super) use check::check;
#[cfg(any(debug_assertions, feature = "gc-trace"))]
pub(crate) use check::check_compiled;

/// A 5.4 / 5.5 key the fast field ops take: a string PUC 5.3+ interns.
pub(super) fn short_key(consts: &[Value], k: u32) -> bool {
    matches!(consts.get(k as usize), Some(Value::Str(s)) if s.len() <= 40)
}

/// The 5.4 / 5.5 immediate forms.
pub(super) enum ImmOp {
    AddI,
    ShrI,
    ShlI,
}

/// The immediate instruction PUC writes for `op` with immediate `c`
/// (`left`: the immediate was the left operand), and the immediate as it
/// writes it; `None`: PUC cannot negate it and goes through a constant or a
/// register.
pub(super) fn imm_op(op: Op, left: bool, c: i32) -> Option<(ImmOp, i32)> {
    let fits = |i: i32| (MIN_SC..=MAX_SC).contains(&i);
    Some(match op {
        Op::Add => (ImmOp::AddI, c),
        Op::Shr => (ImmOp::ShrI, c),
        Op::Shl if left => (ImmOp::ShlI, c),
        Op::Sub if fits(-c) => (ImmOp::AddI, -c),
        Op::Shl if fits(-c) => (ImmOp::ShrI, -c),
        _ => return None,
    })
}

/// For a table op 5.4 / 5.5 have no form for: whether its table is an
/// upvalue and whether its key is a constant, each loaded into a scratch
/// register first.
pub(super) fn temp_key_parts(op: Op, k: bool) -> (bool, bool) {
    let up = matches!(op, Op::GetTabUpR | Op::SetTabUpR | Op::SetTabUpK);
    let const_key = match op {
        Op::GetTabUpR => k,
        Op::SetTabUpR => false,
        _ => true,
    };
    (up, const_key)
}

/// 5.5's `SELF` with a key it has no form for: PUC's parser loads a
/// constant key, moves a key in `a + 1` out of the way, moves the object
/// into `a + 1`, then indexes.
pub(super) struct SelfSteps {
    pub load_key: bool,
    pub move_key: bool,
    pub move_obj: bool,
}

/// `key`: the key's register (a constant's scratch register when `k`).
pub(super) fn self_steps(k: bool, a: u32, b: u32, key: u32) -> SelfSteps {
    SelfSteps {
        load_key: k,
        move_key: key == a + 1,
        move_obj: b != a + 1,
    }
}

/// `SETLIST`'s `C` and the `EXTRAARG` past it (0: none) for element
/// offset `offset` in 5.4 / 5.5.
pub(super) fn modern_setlist(offset: u64, v55: bool) -> (u32, u64) {
    let size = if v55 { 1024 } else { 256 };
    ((offset % size) as u32, offset / size)
}

/// `SetList`'s element offset: luna keeps the whole offset in the
/// `ExtraArg` after it when `k` is set.
fn setlist_offset(p: &Proto, pc: usize) -> u64 {
    let i = p.code[pc];
    if i.k() {
        p.code.get(pc + 1).map_or(0, |x| u64::from(x.ax()))
    } else {
        u64::from(i.c())
    }
}

/// 5.5 `PF_VATAB`: the function's leading `GetVarg` makes the vararg
/// table, which 5.5's `VARARGPREP` builds itself.
pub(super) fn vatab(p: &Proto, d: Dialect) -> bool {
    d == Dialect::V55
        && p.is_vararg
        && p.code
            .first()
            .is_some_and(|i| i.op() == Op::GetVarg && i.a() == p.num_params as u32)
}

/// How many of the last PUC instructions standing for a luna instruction
/// of `op` PUC's parser moves to a line of its own once coded
/// (`luaK_fixline`): a call's, an operator's with its metamethod fallback,
/// a numeric or generic loop's; the operands loaded before them stay on the
/// line they were coded on.
fn moved(op: Op) -> u32 {
    match op {
        Op::Call
        | Op::TailCall
        | Op::Concat
        | Op::Unm
        | Op::BNot
        | Op::Len
        | Op::ErrNNil
        | Op::ForLoop
        | Op::ForLoopF
        | Op::ForLoop55
        | Op::ForLoop55F
        | Op::TForCall
        | Op::TForCall55
        | Op::TForLoop
        | Op::TForLoop55 => 1,
        Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Mod
        | Op::Pow
        | Op::Div
        | Op::IDiv
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr => 2,
        op if op.arith_const_op().is_some() || op.arith_kk_op().is_some() => 2,
        _ => 0,
    }
}

/// PUC's layout of one function, for counting its code as PUC's code
/// generator writes it.
#[derive(Default)]
pub(crate) struct Layout {
    modern: bool,
    v51: bool,
    v55: bool,
    vatab: bool,
    /// the function's encoding, when its widths need the constant indices
    encoded: Option<Box<super::Shape>>,
}

impl Layout {
    pub(crate) fn of(p: &Proto, version: LuaVersion) -> Layout {
        let Some(d) = Dialect::of(version) else {
            return Layout::default();
        };
        let encoded = if p.consts.len() > 255 {
            super::shape_one(p, d).ok().map(Box::new)
        } else {
            None
        };
        Layout {
            modern: d >= Dialect::V54,
            v51: d == Dialect::V51,
            v55: d == Dialect::V55,
            vatab: vatab(p, d),
            encoded,
        }
    }

    /// The function's prologue (5.4+ `VARARGPREP`): `f` with the line of
    /// each instruction.
    #[inline]
    pub(crate) fn prologue(&self, p: &Proto, mut f: impl FnMut(u32)) {
        if let Some(sh) = &self.encoded {
            let n = sh.pc_map.first().copied().unwrap_or(0) as usize;
            sh.lines[..n].iter().for_each(|&l| f(l));
        } else if self.modern && p.is_vararg {
            f(p.line_defined.max(1));
        }
    }

    /// The PUC instructions for luna instruction `pc`: `f` with the line of
    /// each (`line` when the function has no line info); returns how many
    /// constants PUC adds for it.
    #[inline]
    pub(crate) fn code(&self, p: &Proto, pc: usize, line: u32, mut f: impl FnMut(u32)) -> usize {
        if let Some(sh) = &self.encoded {
            let a = sh.pc_map[pc] as usize;
            let b = sh.pc_map[pc + 1] as usize;
            for j in a..b {
                f(sh.lines.get(j).copied().unwrap_or(line));
            }
            return sh.k_added.iter().filter(|&&q| q == pc).count();
        }
        let line = p.lines.get(pc).copied().unwrap_or(line);
        for _ in 0..self.width(p, pc) {
            f(line);
        }
        0
    }

    /// How many of the PUC instructions [`Self::code`] gives for luna
    /// instruction `pc` PUC's parser moves to their line once coded ([`moved`]);
    /// it codes the others on the line the last token read ended on.
    #[inline]
    pub(crate) fn moved(&self, p: &Proto, pc: usize) -> u32 {
        moved(p.code[pc].op())
    }

    /// How many PUC instructions stand for luna instruction `pc` of a
    /// function with fewer than 256 constants.
    #[inline(always)]
    pub(super) fn width(&self, p: &Proto, pc: usize) -> u32 {
        let i = p.code[pc];
        let op = i.op();
        // the instruction before took it in
        if op == Op::ExtraArg {
            return 0;
        }
        if !self.modern {
            return match op {
                Op::SetList => 1 + u32::from(setlist_offset(p, pc) / 50 + 1 > 511),
                Op::Closure if self.v51 => {
                    let child = &p.protos[i.bx() as usize];
                    child.upvals.len().max(1) as u32
                }
                _ => 1,
            };
        }
        match op {
            Op::GetVarg if pc == 0 && self.vatab => 0,
            Op::LoadKx | Op::NewTable => 2,
            Op::GetTabUp if !short_key(&p.consts, i.c()) => 3,
            Op::SetTabUp if !short_key(&p.consts, i.b()) => 3,
            Op::GetField if !short_key(&p.consts, i.c()) => 2,
            Op::SetField if !short_key(&p.consts, i.b()) => 2,
            Op::GetTabUpR | Op::SetTabUpR | Op::SetTabUpK | Op::GetTableK | Op::SetTableK => {
                let (up, const_key) = temp_key_parts(op, i.k());
                1 + u32::from(up) + u32::from(const_key)
            }
            Op::SelfOp if self.v55 && !(i.k() && short_key(&p.consts, i.c())) => {
                let key = if i.k() { u32::from(p.max_stack) } else { i.c() };
                let s = self_steps(i.k(), i.a(), i.b(), key);
                1 + u32::from(s.load_key) + u32::from(s.move_key) + u32::from(s.move_obj)
            }
            Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Mod
            | Op::Pow
            | Op::Div
            | Op::IDiv
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr => 2,
            Op::AddI | Op::SubI | Op::ShrI | Op::ShlI => {
                let op = op.arith_const_op().expect("immediate arithmetic");
                match imm_op(op, i.k(), i.sc()) {
                    Some(_) => 2,
                    // a shift through a register (`LOADI`); the others take
                    // the constant table
                    None if matches!(op, Op::Shl | Op::Shr) => 3,
                    None => 2,
                }
            }
            op if op.arith_const_op().is_some() => 2,
            Op::SetList => 1 + u32::from(modern_setlist(setlist_offset(p, pc), self.v55).1 > 0),
            _ => 1,
        }
    }
}
