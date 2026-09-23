//! Shared machinery for lowering a PUC instruction stream into luna's ISA.
//!
//! Every dialect module decodes its own chunk format into a [`RawProto`]
//! tree and walks its PUC code once, driving a [`Lowering`]. This module
//! owns what the five dialects have in common: register mapping, scratch
//! registers, jump resolution, and the debug tables (line and local-variable
//! info) that must follow the code through the translation.
//!
//! ## What luna's interpreter trusts
//!
//! The dispatch loop reads registers and fetches instructions without bounds
//! checks, on the grounds that its own compiler cannot emit anything else:
//!
//! - every register an instruction touches, including the runs implied by
//!   `Call`, `Return`, `LoadNil`, `Concat`, `SetList`, `Vararg` and the
//!   numeric/generic `for` ops, lies below the proto's `max_stack`;
//! - every jump lands inside the code, and the code cannot fall off its end;
//! - an `ExtraArg` only ever follows `LoadKx` or `SetList` with `k` set.
//!
//! A translator must uphold all three for every chunk PUC's own compiler can
//! produce. It does not re-verify a chunk that PUC's compiler could not have
//! produced: PUC removed its bytecode verifier in 5.2, and luna follows it.
//!
//! luna's operands are also narrower than PUC's in one respect that shapes
//! most lowerings: apart from the constant *keys* of `GetTabUp`, `GetField`,
//! `SetTabUp` and `SetField`, the method-name key of `SelfOp` (when `k` is
//! set) and the right operand of `EqK`, every operand is a register. The `k`
//! bit is ignored by `SetTable`/`SetField`/`SetTabUp`/`SetI` and by the
//! arithmetic ops, so a PUC constant in any other position is first loaded
//! into a scratch register.
//!
//! ## Loop windows
//!
//! luna lays out both kinds of `for` loop the way PUC 5.4 does: four hidden
//! slots (`A..A+3`) with the loop variables after them. PUC 5.1–5.3 generic
//! loops keep three hidden slots, and PUC 5.5 keeps three for both kinds, so
//! their loop variables sit one register lower than luna's ops write them.
//! Each such loop becomes a [`Window`]: across its PUC pcs, every register at
//! or above the window's pivot is renumbered one higher. Registers above the
//! pivot are dead when control enters or leaves the loop, which is why a
//! renumbering confined to the loop's pcs is sound. Windows nest with loops,
//! so the frame grows by the deepest nesting.
//!
//! ## Scratch registers
//!
//! A lowering that needs a register PUC did not allocate takes one above
//! every mapped register. Scratch values live only within the expansion of a
//! single PUC instruction, so each instruction reuses the same slots, and none
//! is ever read after a call (a callee's frame starts inside the caller's).

use crate::runtime::Value;
use crate::runtime::function::{JitProtoState, LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::runtime::string::LuaStr;
use crate::vm::isa::{self, Inst, Op};

/// The "is a constant" bit of a PUC 5.1–5.3 RK operand.
pub(super) const RK_BIT: u32 = 1 << 8;

/// A local-variable record as PUC dumps it: PUC pcs, no register.
pub(super) struct RawLocVar {
    pub name: Box<str>,
    pub start_pc: u32,
    pub end_pc: u32,
}

/// One function as read from a PUC chunk, before its code is translated.
pub(super) struct RawProto {
    pub source: Gc<LuaStr>,
    pub line_defined: u32,
    pub last_line_defined: u32,
    pub num_params: u8,
    pub is_vararg: bool,
    /// PUC 5.1 `VARARG_NEEDSARG`: the function has the implicit `arg` local.
    pub has_compat_vararg_arg: bool,
    /// PUC 5.5 `PF_VATAB`: `VARARGPREP` builds the named vararg parameter
    /// as a real table in register `num_params`.
    pub vararg_table: bool,
    /// PUC's `maxstacksize`.
    pub max_stack: u8,
    pub code: Vec<u32>,
    pub consts: Vec<Value>,
    pub upvals: Vec<UpvalDesc>,
    pub protos: Vec<RawProto>,
    /// Source line per PUC pc; empty for a stripped chunk.
    pub lines: Vec<u32>,
    pub locvars: Vec<RawLocVar>,
}

/// A translated body, ready to become a [`Proto`].
pub(super) struct Lowered {
    pub code: Vec<Inst>,
    pub lines: Vec<u32>,
    pub locvars: Vec<LocVar>,
    pub max_stack: u8,
}

/// Build the proto tree: translate `raw`'s code (the translator may rewrite
/// the upvalue descriptors of `raw.protos`, which depend on the parent's
/// register mapping), then its children.
pub(super) fn build(
    heap: &mut Heap,
    mut raw: RawProto,
    translate: &dyn Fn(&mut RawProto) -> Result<Lowered, String>,
) -> Result<Gc<Proto>, String> {
    let lowered = translate(&mut raw)?;
    let mut protos = Vec::with_capacity(raw.protos.len());
    for child in raw.protos.drain(..) {
        protos.push(build(heap, child, translate)?);
    }
    let env_upval_idx = raw
        .upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);
    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: lowered.code.into_boxed_slice(),
        consts: raw.consts.into_boxed_slice(),
        protos: protos.into_boxed_slice(),
        upvals: raw.upvals.into_boxed_slice(),
        num_params: raw.num_params,
        is_vararg: raw.is_vararg,
        // A PUC 5.5 chunk lists its `(vararg table)` local in `locvars` with
        // the register PUC gave it, so the debug view needs no synthetic one.
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: raw.has_compat_vararg_arg,
        max_stack: lowered.max_stack,
        lines: lowered.lines.into_boxed_slice(),
        source: raw.source,
        line_defined: raw.line_defined,
        last_line_defined: raw.last_line_defined,
        locvars: lowered.locvars.into_boxed_slice(),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
    }))
}

/// PUC pcs `first..=last` of a loop whose registers from `pivot` up are one
/// slot higher in luna's frame (see the module docs).
#[derive(Clone, Copy, Debug)]
pub(super) struct Window {
    pub first: usize,
    pub last: usize,
    pub pivot: u32,
}

/// How a jump's target is encoded once the target's luna pc is known.
#[derive(Clone, Copy, Debug)]
pub(super) enum Jump {
    /// `Jmp`: signed `sJ` from the next pc.
    Jmp,
    /// `ForPrep`: `Bx` = distance to its `ForLoop`.
    ForPrep,
    /// `ForLoop` / `TForLoop`: `Bx` = distance back from the next pc.
    Back,
    /// `TForPrep`: `Bx` = distance from the next pc to its `TForCall`.
    TForPrep,
}

enum Target {
    Puc(usize),
    /// Index into `Lowering::trampolines`.
    Trampoline(usize),
}

struct Fixup {
    at: usize,
    target: Target,
    kind: Jump,
}

/// `Close close; Jmp target`, placed after the function's code.
struct Trampoline {
    close: u32,
    target: usize,
    line: u32,
}

/// Emitter for one function body. See the module docs.
pub(super) struct Lowering {
    dialect: &'static str,
    n_puc: usize,
    windows: Vec<Window>,
    temp_base: u32,
    temps_used: u32,
    next_temp: u32,
    code: Vec<Inst>,
    lines: Vec<u32>,
    /// PUC pc → first luna pc emitted for it (`None`: emitted nothing).
    first: Vec<Option<u32>>,
    fixups: Vec<Fixup>,
    trampolines: Vec<Trampoline>,
    pc: usize,
    line: u32,
}

impl Lowering {
    /// `frame` is PUC's `maxstacksize`; `windows` must already hold every
    /// loop window of the function.
    pub(super) fn new(
        dialect: &'static str,
        n_puc: usize,
        frame: u8,
        windows: Vec<Window>,
    ) -> Lowering {
        let depth = windows
            .iter()
            .map(|w| {
                windows
                    .iter()
                    .filter(|o| o.first <= w.first && w.first <= o.last)
                    .count()
            })
            .max()
            .unwrap_or(0) as u32;
        Lowering {
            dialect,
            n_puc,
            windows,
            temp_base: frame as u32 + depth,
            temps_used: 0,
            next_temp: 0,
            code: Vec::with_capacity(n_puc),
            lines: Vec::with_capacity(n_puc),
            first: vec![None; n_puc],
            fixups: Vec::new(),
            trampolines: Vec::new(),
            pc: 0,
            line: 0,
        }
    }

    /// A translation error, located at the PUC pc being lowered.
    pub(super) fn err(&self, msg: impl std::fmt::Display) -> String {
        format!("{} chunk: {msg} (pc {})", self.dialect, self.pc)
    }

    /// Start lowering PUC pc `pc`, whose source line is `line`.
    pub(super) fn begin(&mut self, pc: usize, line: u32) {
        self.pc = pc;
        self.line = line;
        self.next_temp = 0;
    }

    /// luna register for PUC register `r` at PUC pc `pc`.
    pub(super) fn reg_at(&self, pc: usize, r: u32) -> Result<u32, String> {
        let shift = self
            .windows
            .iter()
            .filter(|w| w.first <= pc && pc <= w.last && r >= w.pivot)
            .count() as u32;
        let m = r + shift;
        if m > isa::MAX_A {
            return Err(self.err(format_args!(
                "register {r} maps to {m}, past luna's 255-register frame"
            )));
        }
        Ok(m)
    }

    /// luna register for PUC register `r` at the current pc.
    pub(super) fn r(&self, r: u32) -> Result<u32, String> {
        self.reg_at(self.pc, r)
    }

    /// luna register for the first of `n` consecutive PUC registers from `r`,
    /// refusing a run that a loop window would split.
    pub(super) fn run(&self, r: u32, n: u32) -> Result<u32, String> {
        let first = self.r(r)?;
        if n > 1 {
            let last = self.r(r + n - 1)?;
            if last - first != n - 1 {
                return Err(self.err(format_args!(
                    "register run {r}..{} straddles a loop's hidden slots",
                    r + n - 1
                )));
            }
        }
        Ok(first)
    }

    /// `v` as an 8-bit luna operand. PUC's 9-bit B/C fields (5.1–5.3) can
    /// hold values luna cannot encode.
    pub(super) fn byte(&self, v: u32, what: &str) -> Result<u32, String> {
        if v > isa::MAX_C {
            return Err(self.err(format_args!("{what} {v} past luna's 8-bit field")));
        }
        Ok(v)
    }

    /// A fresh scratch register, unique within the current PUC instruction.
    pub(super) fn temp(&mut self) -> Result<u32, String> {
        let t = self.temp_base + self.next_temp;
        // max_stack (t + 1) must fit luna's u8 frame size.
        if t >= isa::MAX_A {
            return Err(self.err("scratch register past luna's 255-register frame"));
        }
        self.next_temp += 1;
        self.temps_used = self.temps_used.max(self.next_temp);
        Ok(t)
    }

    pub(super) fn emit(&mut self, inst: Inst) {
        if self.first[self.pc].is_none() {
            self.first[self.pc] = Some(self.code.len() as u32);
        }
        self.code.push(inst);
        self.lines.push(self.line);
    }

    /// Emit a jump-family instruction whose offset is patched once PUC pc
    /// `target` has a luna pc. `inst` carries every field except the offset.
    pub(super) fn jump(&mut self, inst: Inst, kind: Jump, target: i64) -> Result<(), String> {
        let target = self.puc_target(target)?;
        self.fixups.push(Fixup {
            at: self.code.len(),
            target: Target::Puc(target),
            kind,
        });
        self.emit(inst);
        Ok(())
    }

    /// Jump to `target`, closing upvalues from `R[close]` on the way, as a
    /// single luna instruction. A comparison or test skips exactly one luna
    /// instruction, and PUC lets the jump it guards close upvalues (a
    /// `break` out of a loop that captured a local), so the `Close` cannot
    /// sit inline: it goes in a trampoline after the function's code.
    pub(super) fn jump_closing(&mut self, close: u32, target: i64) -> Result<(), String> {
        let target = self.puc_target(target)?;
        self.trampolines.push(Trampoline {
            close,
            target,
            line: self.line,
        });
        self.fixups.push(Fixup {
            at: self.code.len(),
            target: Target::Trampoline(self.trampolines.len() - 1),
            kind: Jump::Jmp,
        });
        self.emit(Inst::isj(Op::Jmp, 0));
        Ok(())
    }

    fn puc_target(&self, target: i64) -> Result<usize, String> {
        if !(0..self.n_puc as i64).contains(&target) {
            return Err(self.err(format_args!("jump target {target} outside the code")));
        }
        Ok(target as usize)
    }

    /// `R[dst] := K[k]`, through `LoadKx` when `k` does not fit `Bx`.
    pub(super) fn load_k(&mut self, dst: u32, k: u32) -> Result<(), String> {
        if k <= isa::MAX_BX {
            self.emit(Inst::iabx(Op::LoadK, dst, k));
        } else if k <= isa::MAX_AX {
            self.emit(Inst::iabc(Op::LoadKx, dst, 0, 0, false));
            self.emit(Inst::iax(Op::ExtraArg, k));
        } else {
            return Err(self.err(format_args!("constant index {k} past luna's limit")));
        }
        Ok(())
    }

    /// A scratch register holding `K[k]`.
    pub(super) fn k_in_temp(&mut self, k: u32) -> Result<u32, String> {
        let t = self.temp()?;
        self.load_k(t, k)?;
        Ok(t)
    }

    /// `R[dst] := R[t][K[k]]`.
    pub(super) fn get_field(&mut self, dst: u32, t: u32, k: u32) -> Result<(), String> {
        if k <= isa::MAX_C {
            self.emit(Inst::iabc(Op::GetField, dst, t, k, false));
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(Inst::iabc(Op::GetTable, dst, t, key, false));
        }
        Ok(())
    }

    /// `R[t][K[k]] := R[v]`.
    pub(super) fn set_field(&mut self, t: u32, k: u32, v: u32) -> Result<(), String> {
        if k <= isa::MAX_B {
            self.emit(Inst::iabc(Op::SetField, t, k, v, false));
        } else {
            let key = self.k_in_temp(k)?;
            self.emit(Inst::iabc(Op::SetTable, t, key, v, false));
        }
        Ok(())
    }

    /// `R[dst] := Upvalue[up][K[k]]`. luna reserves `GetTabUp` for reads of
    /// the global environment and names the upvalue in an error only when
    /// the table was fetched into a register first, as its own compiler
    /// does for any other upvalue.
    pub(super) fn get_tabup(&mut self, dst: u32, up: u32, k: u32, env: bool) -> Result<(), String> {
        if env && k <= isa::MAX_C {
            self.emit(Inst::iabc(Op::GetTabUp, dst, up, k, false));
        } else {
            let t = self.temp()?;
            self.emit(Inst::iabc(Op::GetUpval, t, up, 0, false));
            self.get_field(dst, t, k)?;
        }
        Ok(())
    }

    /// `Upvalue[up][K[k]] := R[v]`; `env` as for [`Self::get_tabup`].
    pub(super) fn set_tabup(&mut self, up: u32, k: u32, v: u32, env: bool) -> Result<(), String> {
        if env && k <= isa::MAX_B {
            self.emit(Inst::iabc(Op::SetTabUp, up, k, v, false));
        } else {
            let t = self.temp()?;
            self.emit(Inst::iabc(Op::GetUpval, t, up, 0, false));
            self.set_field(t, k, v)?;
        }
        Ok(())
    }

    /// `return R[a], ..., R[a+b-2]` (`b == 0`: up to the stack top), in the
    /// form luna's own compiler uses for zero and one value. The three
    /// return ops behave alike in the interpreter; the JIT compiles only the
    /// short forms.
    pub(super) fn ret(&mut self, a: u32, b: u32) -> Result<(), String> {
        let b = self.byte(b, "RETURN B")?;
        let a = self.run(a, b.saturating_sub(1).max(1))?;
        self.emit(match b {
            1 => Inst::iabc(Op::Return0, 0, 0, 0, false),
            2 => Inst::iabc(Op::Return1, a, 0, 0, false),
            _ => Inst::iabc(Op::Return, a, b, 0, false),
        });
        Ok(())
    }

    /// `SetList` storing `n` values from `R[a+1..]` after the first `offset`
    /// array slots.
    pub(super) fn set_list(&mut self, a: u32, n: u32, offset: u64) -> Result<(), String> {
        if n > isa::MAX_B {
            return Err(self.err(format_args!("SETLIST count {n} past luna's 8-bit field")));
        }
        if offset <= isa::MAX_C as u64 {
            self.emit(Inst::iabc(Op::SetList, a, n, offset as u32, false));
        } else if offset <= isa::MAX_AX as u64 {
            self.emit(Inst::iabc(Op::SetList, a, n, 0, true));
            self.emit(Inst::iax(Op::ExtraArg, offset as u32));
        } else {
            return Err(self.err(format_args!("SETLIST offset {offset} past luna's limit")));
        }
        Ok(())
    }

    // ---- RK operands (PUC 5.1–5.3) ----
    //
    // A 9-bit B or C field whose top bit is set names constant `field & 0xFF`
    // instead of a register.

    /// A register holding the RK operand `field`.
    pub(super) fn rk(&mut self, field: u32) -> Result<u32, String> {
        if field & RK_BIT != 0 {
            self.k_in_temp(field & 0xFF)
        } else {
            self.r(field)
        }
    }

    /// `if ((RK(b) <op> RK(c)) ~= k) then pc++`. A constant equality
    /// operand becomes `EqK`'s constant; `==` against a constant is symmetric
    /// (no `__eq` can run), so a constant on the left swaps sides.
    pub(super) fn compare_rk(&mut self, op: Op, k: bool, b: u32, c: u32) -> Result<(), String> {
        let (b_k, c_k) = (b & RK_BIT != 0, c & RK_BIT != 0);
        if op == Op::Eq && (b_k || c_k) {
            let (reg, konst) = if c_k { (b, c) } else { (c, b) };
            let reg = self.rk(reg)?;
            self.emit(Inst::iabc(Op::EqK, reg, konst & 0xFF, 0, k));
            return Ok(());
        }
        let l = self.rk(b)?;
        let r = self.rk(c)?;
        self.emit(Inst::iabc(op, l, r, 0, k));
        Ok(())
    }

    /// `R[dst] := R[t][RK(key)]`.
    pub(super) fn get_table_rk(&mut self, dst: u32, t: u32, key: u32) -> Result<(), String> {
        if key & RK_BIT != 0 {
            self.get_field(dst, t, key & 0xFF)
        } else {
            let key = self.r(key)?;
            self.emit(Inst::iabc(Op::GetTable, dst, t, key, false));
            Ok(())
        }
    }

    /// `R[t][RK(key)] := RK(val)`.
    pub(super) fn set_table_rk(&mut self, t: u32, key: u32, val: u32) -> Result<(), String> {
        let v = self.rk(val)?;
        if key & RK_BIT != 0 {
            self.set_field(t, key & 0xFF, v)
        } else {
            let key = self.r(key)?;
            self.emit(Inst::iabc(Op::SetTable, t, key, v, false));
            Ok(())
        }
    }

    /// `R[a+1] := R[b]; R[a] := R[b][RK(key)]`.
    pub(super) fn self_rk(&mut self, a: u32, b: u32, key: u32) -> Result<(), String> {
        let a = self.run(a, 2)?;
        let b = self.r(b)?;
        if key & RK_BIT != 0 {
            self.emit(Inst::iabc(Op::SelfOp, a, b, key & 0xFF, true));
        } else {
            let key = self.r(key)?;
            self.emit(Inst::iabc(Op::SelfOp, a, b, key, false));
        }
        Ok(())
    }

    /// `R[a] := R[b] .. ... .. R[c]`. luna concatenates in place at the first
    /// operand, so a result register elsewhere takes a `Move`.
    pub(super) fn concat_range(&mut self, a: u32, b: u32, c: u32) -> Result<(), String> {
        if c < b {
            return Err(self.err(format_args!("CONCAT range {b}..{c} is empty")));
        }
        let n = c - b + 1;
        let first = self.run(b, n)?;
        self.emit(Inst::iabc(Op::Concat, first, n, 0, false));
        if a != b {
            let a = self.r(a)?;
            self.emit(Inst::iabc(Op::Move, a, first, 0, false));
        }
        Ok(())
    }

    /// Resolve every jump and remap the debug tables.
    pub(super) fn finish(mut self, raw_locvars: &[RawLocVar]) -> Result<Lowered, String> {
        // Trampolines follow the last instruction, which never falls through.
        let mut tramp_at = Vec::with_capacity(self.trampolines.len());
        for t in std::mem::take(&mut self.trampolines) {
            tramp_at.push(self.code.len() as u32);
            self.code.push(Inst::iabc(Op::Close, t.close, 0, 0, false));
            self.lines.push(t.line);
            self.fixups.push(Fixup {
                at: self.code.len(),
                target: Target::Puc(t.target),
                kind: Jump::Jmp,
            });
            self.code.push(Inst::isj(Op::Jmp, 0));
            self.lines.push(t.line);
        }
        for f in std::mem::take(&mut self.fixups) {
            let t = match f.target {
                Target::Trampoline(i) => tramp_at[i],
                Target::Puc(pc) => match self.first[pc] {
                    Some(t) => t,
                    None => {
                        self.pc = pc;
                        return Err(self.err("jump lands on an instruction that has no luna form"));
                    }
                },
            };
            let (t, at) = (t as i64, f.at as i64);
            let inst = &mut self.code[f.at];
            let op = inst.op();
            let a = inst.a();
            *inst = match f.kind {
                Jump::Jmp => {
                    let sj = t - (at + 1);
                    if !(-(isa::MAX_SJ as i64)..=isa::MAX_SJ as i64).contains(&sj) {
                        return Err(format!(
                            "{} chunk: jump distance {sj} too long",
                            self.dialect
                        ));
                    }
                    Inst::isj(op, sj as i32)
                }
                Jump::ForPrep => Inst::iabx(op, a, bx_distance(self.dialect, t - at)?),
                Jump::Back => Inst::iabx(op, a, bx_distance(self.dialect, at + 1 - t)?),
                Jump::TForPrep => Inst::iabx(op, a, bx_distance(self.dialect, t - (at + 1))?),
            };
        }
        let locvars = self.locvars(raw_locvars)?;
        let max_stack = self.temp_base + self.temps_used;
        if max_stack > u8::MAX as u32 {
            return Err(format!(
                "{} chunk: translated frame needs {max_stack} registers, luna allows 255",
                self.dialect
            ));
        }
        Ok(Lowered {
            code: self.code,
            lines: self.lines,
            locvars,
            max_stack: max_stack as u8,
        })
    }

    /// First luna pc at or after PUC pc `pc` (the code length past the end).
    fn luna_pc(&self, pc: u32) -> u32 {
        self.first
            .iter()
            .skip(pc as usize)
            .find_map(|p| *p)
            .unwrap_or(self.code.len() as u32)
    }

    /// PUC dumps no register for a local: it is the number of locals still
    /// active when it was declared, since PUC gives locals consecutive
    /// registers in declaration order and `locvars` is in that order.
    fn locvars(&self, raw: &[RawLocVar]) -> Result<Vec<LocVar>, String> {
        let mut out = Vec::with_capacity(raw.len());
        for (i, v) in raw.iter().enumerate() {
            let puc_reg = raw[..i]
                .iter()
                .filter(|o| o.start_pc <= v.start_pc && v.start_pc < o.end_pc)
                .count() as u32;
            out.push(LocVar {
                name: v.name.clone(),
                reg: self.reg_at(v.start_pc as usize, puc_reg)?,
                start_pc: self.luna_pc(v.start_pc),
                end_pc: self.luna_pc(v.end_pc),
            });
        }
        Ok(out)
    }
}

/// Per-pc source lines from PUC 5.4/5.5's compressed form
/// (`luaG_getfuncline`): start at `line_defined` and add each signed delta,
/// except that `ABSLINEINFO` (-128) takes the line of the `abslineinfo`
/// entry recorded for that pc. Empty for a stripped chunk.
pub(super) fn rle_lines(
    dialect: &str,
    deltas: &[u8],
    abs: &[(u32, u32)],
    line_defined: u32,
    n_code: usize,
) -> Result<Vec<u32>, String> {
    if deltas.is_empty() {
        return Ok(Vec::new());
    }
    if deltas.len() != n_code {
        return Err(format!(
            "{dialect} chunk: {} line entries for {n_code} instructions",
            deltas.len()
        ));
    }
    let mut out = Vec::with_capacity(n_code);
    let mut line = line_defined as i64;
    let mut abs = abs.iter();
    for (pc, &d) in deltas.iter().enumerate() {
        if d as i8 == -128 {
            match abs.next() {
                Some(&(apc, aline)) if apc as usize == pc => line = aline as i64,
                _ => return Err(format!("{dialect} chunk: no absolute line for pc {pc}")),
            }
        } else {
            line += (d as i8) as i64;
        }
        out.push(u32::try_from(line).map_err(|_| format!("{dialect} chunk: line {line}"))?);
    }
    Ok(out)
}

fn bx_distance(dialect: &str, d: i64) -> Result<u32, String> {
    if !(0..=isa::MAX_BX as i64).contains(&d) {
        return Err(format!(
            "{dialect} chunk: loop jump distance {d} out of luna's range"
        ));
    }
    Ok(d as u32)
}

/// A bare function around `code`, for translator unit tests.
#[cfg(test)]
pub(super) fn test_proto(
    heap: &mut Heap,
    code: Vec<u32>,
    consts: Vec<Value>,
    frame: u8,
) -> RawProto {
    RawProto {
        source: heap.intern(b"=test"),
        line_defined: 0,
        last_line_defined: 0,
        num_params: 0,
        is_vararg: false,
        has_compat_vararg_arg: false,
        vararg_table: false,
        max_stack: frame,
        code,
        consts,
        upvals: Vec::new(),
        protos: Vec::new(),
        lines: Vec::new(),
        locvars: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowering(n: usize, frame: u8, windows: Vec<Window>) -> Lowering {
        Lowering::new("test", n, frame, windows)
    }

    #[test]
    fn window_moves_registers_from_its_pivot_up_inside_its_pcs() {
        let w = Window {
            first: 2,
            last: 5,
            pivot: 3,
        };
        let lw = lowering(8, 10, vec![w]);
        assert_eq!(lw.reg_at(1, 4).unwrap(), 4, "before the loop");
        assert_eq!(lw.reg_at(3, 2).unwrap(), 2, "below the pivot");
        assert_eq!(lw.reg_at(3, 3).unwrap(), 4, "the first loop variable");
        assert_eq!(lw.reg_at(6, 4).unwrap(), 4, "after the loop");
    }

    #[test]
    fn nested_windows_add_up_and_push_scratch_registers_above_both() {
        let outer = Window {
            first: 0,
            last: 9,
            pivot: 3,
        };
        let inner = Window {
            first: 2,
            last: 6,
            pivot: 7,
        };
        let mut lw = lowering(10, 12, vec![outer, inner]);
        assert_eq!(lw.reg_at(4, 7).unwrap(), 9);
        assert_eq!(lw.reg_at(4, 5).unwrap(), 6);
        lw.begin(4, 0);
        assert_eq!(lw.temp().unwrap(), 14, "frame 12 + nesting depth 2");
    }

    #[test]
    fn a_run_split_by_a_window_is_refused() {
        let w = Window {
            first: 0,
            last: 3,
            pivot: 3,
        };
        let lw = lowering(4, 10, vec![w]);
        assert!(lw.run(1, 3).is_err());
        assert_eq!(lw.run(3, 3).unwrap(), 4);
    }

    #[test]
    fn a_local_takes_the_register_after_the_locals_live_at_its_start() {
        let raw = [
            ("a", 0, 10),
            ("b", 2, 5),
            ("c", 6, 10), // b is gone by now: c reuses its register
            ("d", 7, 9),
        ]
        .map(|(n, s, e)| RawLocVar {
            name: n.into(),
            start_pc: s,
            end_pc: e,
        });
        let mut lw = lowering(10, 10, Vec::new());
        for pc in 0..10 {
            lw.begin(pc, 0);
            lw.emit(Inst::iabc(Op::Move, 0, 0, 0, false));
        }
        let regs: Vec<u32> = lw
            .finish(&raw)
            .unwrap()
            .locvars
            .iter()
            .map(|v| v.reg)
            .collect();
        assert_eq!(regs, [0, 1, 1, 2]);
    }

    #[test]
    fn a_guarded_closing_jump_stays_one_instruction() {
        let mut lw = lowering(3, 4, Vec::new());
        lw.begin(0, 0);
        lw.emit(Inst::iabc(Op::Test, 0, 0, 0, true));
        lw.begin(1, 0);
        lw.jump_closing(2, 0).unwrap();
        lw.begin(2, 0);
        lw.emit(Inst::iabc(Op::Return0, 0, 0, 0, false));
        let code = lw.finish(&[]).unwrap().code;
        assert_eq!(code.len(), 5);
        assert_eq!(code[1].op(), Op::Jmp);
        assert_eq!(
            1 + 1 + code[1].sj(),
            3,
            "the test's jump goes to the trampoline"
        );
        assert_eq!((code[3].op(), code[3].a()), (Op::Close, 2));
        assert_eq!(code[4].op(), Op::Jmp);
        assert_eq!(4 + 1 + code[4].sj(), 0, "the trampoline goes to the target");
    }

    #[test]
    fn a_constant_index_past_bx_loads_through_extraarg() {
        let mut lw = lowering(1, 2, Vec::new());
        lw.begin(0, 0);
        lw.load_k(1, isa::MAX_BX + 1).unwrap();
        let code = lw.finish(&[]).unwrap().code;
        assert_eq!(code[0].op(), Op::LoadKx);
        assert_eq!(
            (code[1].op(), code[1].ax()),
            (Op::ExtraArg, isa::MAX_BX + 1)
        );
    }

    #[test]
    fn concat_into_another_register_moves_the_result() {
        let mut lw = lowering(1, 8, Vec::new());
        lw.begin(0, 0);
        lw.concat_range(5, 2, 4).unwrap();
        let code = lw.finish(&[]).unwrap().code;
        assert_eq!((code[0].op(), code[0].a(), code[0].b()), (Op::Concat, 2, 3));
        assert_eq!((code[1].op(), code[1].a(), code[1].b()), (Op::Move, 5, 2));
    }

    #[test]
    fn set_list_offsets_past_c_use_extraarg() {
        let mut lw = lowering(1, 8, Vec::new());
        lw.begin(0, 0);
        lw.set_list(1, 3, 300).unwrap();
        let code = lw.finish(&[]).unwrap().code;
        assert!(code[0].k());
        assert_eq!((code[1].op(), code[1].ax()), (Op::ExtraArg, 300));
    }

    #[test]
    fn abslineinfo_sets_the_line_instead_of_adding() {
        // pc0 +1, pc1 absolute 500, pc2 +2
        let lines = rle_lines("t", &[1, 0x80, 2], &[(1, 500)], 10, 3).unwrap();
        assert_eq!(lines, vec![11, 500, 502]);
    }
}
