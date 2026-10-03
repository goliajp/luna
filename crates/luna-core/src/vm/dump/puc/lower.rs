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
//! produce. A chunk PUC's compiler could not have produced is caught after
//! lowering by the loader's verifier (`super::super::verify`), which checks
//! these invariants on every loaded function whatever its format.
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

pub(super) use super::lines::rle_lines;
use crate::runtime::Value;
use crate::runtime::function::{JitProtoState, LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::runtime::string::LuaStr;
use crate::vm::isa::{self, Inst, Op};

pub(super) use super::encode::{RK_BIT, enc_abc, enc_abx, enc_asbx, enc_ax, enc_sj};

/// A local-variable record as PUC dumps it: PUC pcs, no register.
pub(super) struct RawLocVar {
    pub name: crate::runtime::DebugName,
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
    let mut lowered = translate(&mut raw)?;
    // a stripped chunk has no line info; the translated one must not
    // invent line 0 for every instruction
    if raw.lines.is_empty() {
        lowered.lines.clear();
    }
    let mut protos = Vec::with_capacity(raw.protos.len());
    for child in raw.protos.drain(..) {
        protos.push(build(heap, child, translate)?);
    }
    crate::runtime::function_close::mark_closing_returns(&mut lowered.code, &protos);
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
        inlined_protos: std::cell::RefCell::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        has_dispatchable_trace: std::cell::Cell::new(false),
        trace_heads: std::cell::Cell::new(
            [crate::runtime::function::TRACE_HEADS_NONE; crate::runtime::function::TRACE_HEADS_CAP],
        ),
        trace_call_head_settled: std::cell::Cell::new(false),
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
    /// which constants are strings: only those may be the key of `GetField`,
    /// `SetField`, `GetTabUp`, `SetTabUp` and a `k` `SelfOp`, which the
    /// interpreter reads as strings without looking
    kstr: Vec<bool>,
}

impl Lowering {
    /// `frame` is PUC's `maxstacksize`; `windows` must already hold every
    /// loop window of the function.
    pub(super) fn new(
        dialect: &'static str,
        n_puc: usize,
        frame: u8,
        windows: Vec<Window>,
        consts: &[Value],
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
            kstr: consts.iter().map(|v| matches!(v, Value::Str(_))).collect(),
        }
    }

    /// Whether `K[k]` is a string (see `kstr`).
    pub(super) fn is_kstr(&self, k: u32) -> bool {
        self.kstr.get(k as usize).copied().unwrap_or(false)
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
        self.emit(enc_sj(Op::Jmp, 0)?);
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
            self.emit(enc_abx(Op::LoadK, dst, k)?);
        } else if k <= isa::MAX_AX {
            self.emit(enc_abc(Op::LoadKx, dst, 0, 0, false)?);
            self.emit(enc_ax(Op::ExtraArg, k)?);
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

    /// `return R[a], ..., R[a+b-2]` (`b == 0`: up to the stack top), in the
    /// form luna's own compiler uses for zero and one value. The three
    /// return ops behave alike in the interpreter; the JIT compiles only the
    /// short forms.
    pub(super) fn ret(&mut self, a: u32, b: u32) -> Result<(), String> {
        let b = self.byte(b, "RETURN B")?;
        let a = self.run(a, b.saturating_sub(1).max(1))?;
        self.emit(match b {
            1 => enc_abc(Op::Return0, 0, 0, 0, false)?,
            2 => enc_abc(Op::Return1, a, 0, 0, false)?,
            _ => enc_abc(Op::Return, a, b, 0, false)?,
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
            self.emit(enc_abc(Op::SetList, a, n, offset as u32, false)?);
        } else if offset <= isa::MAX_AX as u64 {
            self.emit(enc_abc(Op::SetList, a, n, 0, true)?);
            self.emit(enc_ax(Op::ExtraArg, offset as u32)?);
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
            self.emit(enc_abc(Op::EqK, reg, konst & 0xFF, 0, k)?);
            return Ok(());
        }
        let l = self.rk(b)?;
        let r = self.rk(c)?;
        self.emit(enc_abc(op, l, r, 0, k)?);
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
        self.emit(enc_abc(Op::Concat, first, n, 0, false)?);
        if a != b {
            let a = self.r(a)?;
            self.emit(enc_abc(Op::Move, a, first, 0, false)?);
        }
        Ok(())
    }
}

mod finish;

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(super) use tests::test_proto;
