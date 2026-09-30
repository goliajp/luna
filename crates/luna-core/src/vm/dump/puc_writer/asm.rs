//! Per-function assembly state shared by the dialect encoders.
//!
//! An encoder walks a luna function one instruction at a time and emits
//! the PUC instructions that stand for it. [`Asm`] owns what that needs
//! across dialects: the register renumbering of loop windows, scratch
//! registers above the frame, the luna-pc → PUC-pc map that jumps and
//! debug records are rewritten through, and the constant table, which an
//! older dialect may have to extend (its `LOADK` replaces luna's
//! immediates).

use crate::runtime::Value;
use crate::runtime::function::Proto;
use crate::vm::isa::{Inst, Op};

pub(super) type Res<T> = Result<T, String>;

/// Luna pcs `first..=last` in which every luna register from `pivot` up
/// sits `delta` slots away in PUC's frame. A loop whose PUC layout has one
/// hidden slot fewer than luna's shifts down by one, and the slot it drops
/// (`pivot - 1`) must not be named inside the loop.
#[derive(Clone, Copy)]
pub(super) struct Window {
    pub first: usize,
    pub last: usize,
    pub pivot: u32,
    pub delta: i32,
}

/// How a jump-family instruction stores the distance to its target.
#[derive(Clone, Copy)]
pub(super) enum Dist {
    /// 5.4/5.5 `sJ`: target - (pc + 1), 25 bits.
    SJ,
    /// 5.4/5.5 `Bx` forward: target - (pc + 1).
    BxFwd,
    /// 5.4/5.5 `Bx` backward: (pc + 1) - target.
    BxBack,
    /// 5.1–5.3 `sBx`: target - (pc + 1), 18 bits.
    SBx,
}

struct Fixup {
    at: usize,
    /// a luna pc; the jump lands on the first PUC instruction emitted for it
    target: usize,
    dist: Dist,
}

pub(super) struct Asm<'p> {
    pub p: &'p Proto,
    dialect: &'static str,
    windows: Vec<Window>,
    temp_base: u32,
    next_temp: u32,
    temps_used: u32,
    pub code: Vec<u32>,
    lines: Vec<u32>,
    /// luna pc → first PUC pc emitted for it (`None`: nothing emitted)
    first: Vec<Option<u32>>,
    fixups: Vec<Fixup>,
    pub consts: Vec<Value>,
    /// luna pcs some jump, loop edge or skip lands on
    targets: Vec<bool>,
    pc: usize,
    line: u32,
}

/// What assembling a function body produced.
pub(super) struct Body {
    pub code: Vec<u32>,
    pub lines: Vec<u32>,
    pub consts: Vec<Value>,
    pub frame: u32,
    /// luna pc → PUC pc, one entry past the last luna pc
    pub pc_map: Vec<u32>,
}

impl<'p> Asm<'p> {
    /// `frame` is the PUC frame the renumbered registers need; scratch
    /// registers go above it.
    pub(super) fn new(
        p: &'p Proto,
        dialect: &'static str,
        windows: Vec<Window>,
        frame: u32,
    ) -> Self {
        Asm {
            p,
            dialect,
            windows,
            temp_base: frame,
            next_temp: 0,
            temps_used: 0,
            code: Vec::with_capacity(p.code.len() + 4),
            lines: Vec::with_capacity(p.code.len() + 4),
            first: vec![None; p.code.len()],
            fixups: Vec::new(),
            consts: p.consts.to_vec(),
            targets: jump_targets(p),
            pc: 0,
            line: 0,
        }
    }

    pub(super) fn err(&self, msg: impl std::fmt::Display) -> String {
        format!(
            "{}: {msg} (function at line {}, instruction {})",
            self.dialect,
            self.p.line_defined,
            self.pc + 1
        )
    }

    /// Start encoding luna pc `pc`.
    pub(super) fn begin(&mut self, pc: usize) {
        self.pc = pc;
        self.line = self.p.lines.get(pc).copied().unwrap_or(0);
        self.next_temp = 0;
    }

    pub(super) fn pc(&self) -> usize {
        self.pc
    }

    /// Whether control can reach luna pc `pc` other than by falling
    /// through from `pc - 1`.
    pub(super) fn is_target(&self, pc: usize) -> bool {
        self.targets.get(pc).copied().unwrap_or(false)
    }

    pub(super) fn inst(&self, pc: usize) -> Option<Inst> {
        self.p.code.get(pc).copied()
    }

    /// PUC register for luna register `r` at luna pc `pc`.
    pub(super) fn reg_at(&self, pc: usize, r: u32) -> Res<u32> {
        let mut m = r as i64;
        for w in self
            .windows
            .iter()
            .filter(|w| w.first <= pc && pc <= w.last)
        {
            if r >= w.pivot {
                m += w.delta as i64;
            } else if w.delta < 0 && r + 1 == w.pivot {
                return Err(self.err(format_args!(
                    "register {r} is a loop slot PUC does not have"
                )));
            }
        }
        u32::try_from(m)
            .ok()
            .filter(|&m| m <= 255)
            .ok_or_else(|| self.err(format_args!("register {r} maps outside the frame")))
    }

    pub(super) fn r(&self, r: u32) -> Res<u32> {
        self.reg_at(self.pc, r)
    }

    /// The first of `n` consecutive registers from luna `r`, refusing a run
    /// that a window would split.
    pub(super) fn run(&self, r: u32, n: u32) -> Res<u32> {
        let first = self.r(r)?;
        if n > 1 && self.r(r + n - 1)? != first + n - 1 {
            return Err(self.err(format_args!(
                "register run {r}..{} crosses a loop's slots",
                r + n - 1
            )));
        }
        Ok(first)
    }

    /// A scratch register, unique within the current luna instruction.
    pub(super) fn temp(&mut self) -> Res<u32> {
        let t = self.temp_base + self.next_temp;
        if t >= 255 {
            return Err(self.err("no register left for a scratch value"));
        }
        self.next_temp += 1;
        self.temps_used = self.temps_used.max(self.next_temp);
        Ok(t)
    }

    pub(super) fn emit(&mut self, word: u32) {
        if self.first[self.pc].is_none() {
            self.first[self.pc] = Some(self.code.len() as u32);
        }
        self.code.push(word);
        self.lines.push(self.line);
    }

    /// An instruction that belongs to no luna instruction (a prologue),
    /// on line `line`.
    pub(super) fn emit_prologue(&mut self, word: u32, line: u32) {
        self.code.push(word);
        self.lines.push(line);
    }

    /// Emit `word`, whose distance field is filled in once luna pc
    /// `target` has a PUC pc.
    pub(super) fn jump(&mut self, word: u32, dist: Dist, target: i64) -> Res<()> {
        if !(0..self.p.code.len() as i64).contains(&target) {
            return Err(self.err(format_args!("jump target {target} outside the code")));
        }
        self.fixups.push(Fixup {
            at: self.code.len(),
            target: target as usize,
            dist,
        });
        self.emit(word);
        Ok(())
    }

    /// Index of constant `v`, appended when the table lacks it.
    pub(super) fn konst(&mut self, v: Value) -> u32 {
        let same = |k: &Value| match (k, &v) {
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        };
        match self.consts.iter().position(same) {
            Some(i) => i as u32,
            None => {
                self.consts.push(v);
                self.consts.len() as u32 - 1
            }
        }
    }

    /// Constant `k` when it is a string PUC 5.3+ interns (the fast field
    /// ops of 5.4/5.5 take only those).
    pub(super) fn short_str(&self, k: u32) -> bool {
        matches!(self.consts.get(k as usize), Some(Value::Str(s)) if s.len() <= 40)
    }

    /// PUC pc of luna pc `pc`: the first instruction emitted for it or for
    /// a later one (the code length past the end).
    fn puc_pc(&self, pc: usize) -> u32 {
        self.first
            .iter()
            .skip(pc)
            .find_map(|p| *p)
            .unwrap_or(self.code.len() as u32)
    }

    /// Luna's line table is empty for a function loaded from a stripped
    /// chunk; PUC then gets none either.
    pub(super) fn finish(mut self) -> Res<Body> {
        for f in std::mem::take(&mut self.fixups) {
            let t = self.puc_pc(f.target) as i64;
            let at = f.at as i64;
            let d = match f.dist {
                Dist::BxBack => at + 1 - t,
                _ => t - (at + 1),
            };
            let w = self.code[f.at];
            self.code[f.at] = match f.dist {
                Dist::SJ if d.abs() < (1 << 24) => w | (((d + (1 << 24) - 1) as u32) << 7),
                Dist::BxFwd | Dist::BxBack if (0..1 << 17).contains(&d) => w | ((d as u32) << 15),
                Dist::SBx if d.abs() < (1 << 17) => w | (((d + (1 << 17) - 1) as u32) << 14),
                _ => return Err(format!("{}: jump distance {d} does not fit", self.dialect)),
            };
        }
        let pc_map = (0..=self.p.code.len()).map(|pc| self.puc_pc(pc)).collect();
        let lines = if self.p.lines.is_empty() {
            Vec::new()
        } else {
            self.lines
        };
        Ok(Body {
            code: self.code,
            lines,
            consts: self.consts,
            frame: self.temp_base + self.temps_used,
            pc_map,
        })
    }
}

fn jump_targets(p: &Proto) -> Vec<bool> {
    let n = p.code.len();
    let mut t = vec![false; n + 2];
    let mut mark = |pc: i64| {
        if (0..t.len() as i64).contains(&pc) {
            t[pc as usize] = true;
        }
    };
    for (pc, i) in p.code.iter().enumerate() {
        let (pc, bx) = (pc as i64, i.bx() as i64);
        match i.op() {
            Op::Jmp => mark(pc + 1 + i.sj() as i64),
            Op::ForPrep => {
                mark(pc + bx);
                mark(pc + bx + 1);
            }
            Op::ForLoop | Op::TForLoop => mark(pc + 1 - bx),
            Op::TForPrep => mark(pc + 1 + bx),
            op if op == Op::LFalseSkip || op.is_test() => mark(pc + 2),
            _ => {}
        }
    }
    t
}

/// A luna instruction's operands, decoded once.
#[derive(Clone, Copy)]
pub(super) struct L {
    pub op: Op,
    pub a: u32,
    pub b: u32,
    pub c: u32,
    pub k: bool,
    pub bx: u32,
    pub sbx: i32,
    pub sj: i64,
}

impl L {
    pub(super) fn of(i: Inst) -> L {
        L {
            op: i.op(),
            a: i.a(),
            b: i.b(),
            c: i.c(),
            k: i.k(),
            bx: i.bx(),
            sbx: i.sbx(),
            sj: i.sj() as i64,
        }
    }
}

/// `SetList`'s element offset (luna keeps the whole offset in the
/// `ExtraArg` when `k` is set) and whether that `ExtraArg` follows.
pub(super) fn setlist_offset(asm: &Asm, l: L) -> Res<u64> {
    if !l.k {
        return Ok(l.c as u64);
    }
    match asm.inst(asm.pc() + 1) {
        Some(x) if x.op() == Op::ExtraArg => Ok(x.ax() as u64),
        _ => Err(asm.err("SetList without its ExtraArg")),
    }
}

/// The array-size hint PUC's parser gives a constructor: luna counts an
/// open last item (a call or `...` stored by a `SetList` with `B = 0`),
/// PUC does not.
pub(super) fn array_hint(p: &Proto, pc: usize, reg: u32, narr: u32) -> u32 {
    for i in &p.code[pc + 1..] {
        match i.op() {
            Op::NewTable if i.a() == reg => break,
            Op::SetList if i.a() == reg && i.b() == 0 => return narr.saturating_sub(1),
            _ => {}
        }
    }
    narr
}

/// Loop windows of `p`: each generic `for` (and, with `numeric`, each
/// numeric one) from its prep to its loop op, with loop variables that sit
/// one register lower in PUC than in luna from luna register `A + pivot`.
pub(super) fn loop_windows(p: &Proto, generic: u32, numeric: Option<u32>) -> Res<Vec<Window>> {
    let code = &p.code;
    let mut out = Vec::new();
    for (pc, i) in code.iter().enumerate() {
        let (last, pivot) = match i.op() {
            Op::TForPrep => (pc + 2 + i.bx() as usize, generic),
            Op::ForPrep => match numeric {
                Some(n) => (pc + i.bx() as usize, n),
                None => continue,
            },
            _ => continue,
        };
        let want = if i.op() == Op::ForPrep {
            Op::ForLoop
        } else {
            Op::TForLoop
        };
        if code.get(last).map(|x| x.op()) != Some(want) {
            return Err(format!("loop prep at pc {} without its loop op", pc + 1));
        }
        out.push(Window {
            first: pc,
            last,
            pivot: i.a() + pivot,
            delta: -1,
        });
    }
    Ok(out)
}
