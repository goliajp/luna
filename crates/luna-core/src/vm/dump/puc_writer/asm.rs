//! Per-function assembly state shared by the dialect encoders.
//!
//! An encoder walks a luna function one instruction at a time and emits
//! the PUC instructions that stand for it. [`Asm`] owns what that needs
//! across dialects: scratch registers above the frame (only code loaded
//! from another dialect's chunk needs one), the luna-pc → PUC-pc map that
//! jumps and debug records are rewritten through, and the constant table,
//! which an older dialect may have to extend (its `LOADK` replaces luna's
//! immediates). luna's registers are PUC's: an encoder writes them as they
//! are.

use crate::compiler::const_map::{DumpConstMap, add_const, const_map_of};
use crate::compiler::num_first::{first_nums, num_key};
use crate::runtime::Value;
use crate::runtime::function::Proto;
use crate::version::LuaVersion;
use crate::vm::isa::{Inst, Op};

pub(super) type Res<T> = Result<T, String>;

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
    temp_base: u32,
    next_temp: u32,
    temps_used: u32,
    pub code: Vec<u32>,
    /// the PUC instructions emitted; with `shape_only` their words are not
    /// kept, only how many there are and their lines
    n: u32,
    pub shape_only: bool,
    lines: Vec<u32>,
    /// luna pc → first PUC pc emitted for it (`None`: nothing emitted)
    first: Vec<Option<u32>>,
    fixups: Vec<Fixup>,
    pub consts: Vec<Value>,
    /// the dialect and its scanner table over `consts` (see `const_map`)
    /// built when the encoding first adds a constant
    pub kmap: (LuaVersion, Option<DumpConstMap>),
    /// 5.3: each number's first index in `consts` (see `konst`)
    kfirst: Option<DumpConstMap>,
    /// the luna pc of each constant the encoding adds
    pub k_added: Vec<usize>,
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
    /// the luna pc of each constant the encoding added
    pub k_added: Vec<usize>,
}

impl<'p> Asm<'p> {
    /// Scratch registers go above the function's frame.
    pub(super) fn new(p: &'p Proto, dialect: &'static str) -> Self {
        Asm {
            p,
            dialect,
            temp_base: p.max_stack as u32,
            next_temp: 0,
            temps_used: 0,
            code: Vec::new(),
            n: 0,
            shape_only: false,
            lines: Vec::with_capacity(p.code.len() + 4),
            first: vec![None; p.code.len()],
            fixups: Vec::new(),
            consts: p.consts.to_vec(),
            kmap: (LuaVersion::Lua54, None),
            kfirst: None,
            k_added: Vec::new(),
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

    pub(super) fn inst(&self, pc: usize) -> Option<Inst> {
        self.p.code.get(pc).copied()
    }

    /// The PUC register of luna register `r`: the same one.
    pub(super) fn r(&self, r: u32) -> Res<u32> {
        if r > 255 {
            return Err(self.err(format_args!("register {r} outside the frame")));
        }
        Ok(r)
    }

    /// The first of `n` consecutive registers from luna `r`.
    pub(super) fn run(&self, r: u32, n: u32) -> Res<u32> {
        self.r(r + n.saturating_sub(1))?;
        self.r(r)
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
            self.first[self.pc] = Some(self.n);
        }
        self.push(word, self.line);
    }

    /// An instruction that belongs to no luna instruction (a prologue),
    /// on line `line`.
    pub(super) fn emit_prologue(&mut self, word: u32, line: u32) {
        self.push(word, line);
    }

    fn push(&mut self, word: u32, line: u32) {
        if !self.shape_only {
            self.code.push(word);
        }
        self.n += 1;
        self.lines.push(line);
    }

    /// Emit `word`, whose distance field is filled in once luna pc
    /// `target` has a PUC pc.
    pub(super) fn jump(&mut self, word: u32, dist: Dist, target: i64) -> Res<()> {
        if !(0..self.p.code.len() as i64).contains(&target) {
            return Err(self.err(format_args!("jump target {target} outside the code")));
        }
        self.fixups.push(Fixup {
            at: self.n as usize,
            target: target as usize,
            dist,
        });
        self.emit(word);
        Ok(())
    }

    /// Index of constant `v`, appended where PUC's code generator would.
    pub(super) fn konst(&mut self, v: Value) -> u32 {
        // 5.3 repeats a number another function took the key of; the
        // compiler keeps an immediate only for its first constant
        let key = num_key(&v).filter(|_| self.kmap.0 == LuaVersion::Lua53);
        if let Some(key) = &key {
            let first = self.kfirst.get_or_insert_with(|| first_nums(&self.consts));
            if let Some(&i) = first.get(key) {
                return i;
            }
        }
        let (ver, map) = &mut self.kmap;
        let map = map.get_or_insert_with(|| const_map_of(*ver, &self.consts));
        let n = self.consts.len();
        let k = add_const(*ver, &mut self.consts, map, v);
        if self.consts.len() > n {
            self.k_added.push(self.pc);
            if let (Some(key), Some(first)) = (key, &mut self.kfirst) {
                first.entry(key).or_insert(k);
            }
        }
        k
    }

    /// Constant `k` when it is a string PUC 5.3+ interns (the fast field
    /// ops of 5.4/5.5 take only those).
    pub(super) fn short_str(&self, k: u32) -> bool {
        matches!(self.consts.get(k as usize), Some(Value::Str(s)) if s.len() <= 40)
    }

    /// Luna's line table is empty for a function loaded from a stripped
    /// chunk; PUC then gets none either.
    pub(super) fn finish(mut self) -> Res<Body> {
        // each luna pc's PUC pc: the first instruction emitted for it or
        // for a later one (the code length past the end)
        let mut pc_map = vec![self.n; self.p.code.len() + 1];
        for pc in (0..self.p.code.len()).rev() {
            pc_map[pc] = self.first[pc].unwrap_or(pc_map[pc + 1]);
        }
        for f in std::mem::take(&mut self.fixups) {
            let t = i64::from(pc_map[f.target]);
            let at = f.at as i64;
            let d = match f.dist {
                Dist::BxBack => at + 1 - t,
                _ => t - (at + 1),
            };
            let w = self.code.get(f.at).copied().unwrap_or(0);
            let patched = match f.dist {
                Dist::SJ if d.abs() < (1 << 24) => w | (((d + (1 << 24) - 1) as u32) << 7),
                Dist::BxFwd | Dist::BxBack if (0..1 << 17).contains(&d) => w | ((d as u32) << 15),
                Dist::SBx if d.abs() < (1 << 17) => w | (((d + (1 << 17) - 1) as u32) << 14),
                _ => return Err(format!("{}: jump distance {d} does not fit", self.dialect)),
            };
            if !self.shape_only {
                self.code[f.at] = patched;
            }
        }
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
            k_added: self.k_added,
        })
    }
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
            sj: i.jump_offset() as i64,
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
