//! The compute-path whitelist and control-flow plan for one chunk.

use super::flow;
use crate::operands::{int_arith, int_compare, is_arith, is_compare};
use crate::upval_roles::determine_getupval_roles;
use luna_core::runtime::{Value, function::Proto};
use luna_core::vm::isa::{Inst, Op};

// Compute-path whitelist notes (consumption itself happens inside
// `ChunkPlan::from_proto`):
//
// - `Mod` / `ModK` use Lua semantics (floor mod, sign matches
//   divisor), not C's truncating srem. `Op::Div` / `DivK` are
//   intentionally **excluded** because Lua 5.4 `/` always returns a
//   float regardless of operand types; emitting it as int sdiv would
//   silently mis-compile `2 / 3` (Lua → 0.666…, the int chunk would
//   return 0). Div needs float-reg support (`ret_is_float=true` +
//   `f64::from_bits` reinterpret).
// - A comparison + `Jmp` becomes a single LLVM `condbr`; bare `Jmp`
//   becomes `br`. Multiple reachable returns are tolerated (must agree
//   on `Return0`-vs-`Return1` shape).
// - The `K` forms read an integer constant; `LoadK` itself stays
//   outside the compute whitelist.

/// Arity cap shared with the Cranelift backend.
const MAX_JIT_ARITY: u32 = 16;

/// Whitelisted op set + control-flow plan that the
/// compute lowerer understands. Built by [`ChunkPlan::from_proto`];
/// `None` when the proto falls outside the supported whitelist.
///
/// The plan is the full reach-analysed bytecode + a per-PC vector of
/// basic-block start markers (every entry PC, every jump target, every
/// PC immediately following a terminator). With branching ops in scope
/// "reachable" is not a "sequential prefix"; we
/// trace edges from PC 0 and mark every visited PC for emit.
pub(super) struct ChunkPlan<'a> {
    /// Full chunk code (not truncated). The reach map (`reachable`)
    /// tells the lowerer which PCs to emit; unreachable PCs are
    /// skipped entirely.
    pub(super) code: &'a [Inst],
    /// The constant table the `K` forms read.
    pub(super) consts: &'a [Value],
    /// Number of i64 register slots to alloca on entry.
    pub(super) num_regs: u32,
    /// Number of positional i64 args the JIT entry
    /// accepts. `0` keeps the plain `extern "C" fn() -> i64`
    /// signature; `> 0` widens to `fn(i64, …, i64) -> i64` and the
    /// entry BB loads each `function.get_nth_param(i)` into `regs[i]`
    /// so the lowerer sees param 0..N-1 as live register sources.
    pub(super) num_params: u32,
    /// True ↔ all reachable returns are `Return1`; false ↔ all are
    /// `Return0`. A proto whose reachable set mixes the two bails
    /// (would need a polymorphic dispatcher contract — out of scope).
    pub(super) returns_one: bool,
    /// `true` at every PC that starts a new basic block. PC 0 always
    /// starts a BB; jump targets, the PC immediately after every
    /// terminator (`Return0|Return1|Jmp`), and the fall-through PC
    /// after a `Lt|Le|Eq` (which is `pc+2` because the paired Jmp at
    /// `pc+1` is consumed by the condbr) all start BBs too.
    pub(super) bb_starts: Vec<bool>,
    /// `true` at every PC that holds a `Jmp` consumed by a preceding
    /// `Lt|Le|Eq` (i.e. the Jmp is folded into the condbr emit and
    /// must not be lowered as a separate op).
    pub(super) consumed_jmp: Vec<bool>,
    /// `true` at every PC reachable via the control-flow trace from
    /// PC 0. Unreachable PCs (e.g. the trailing implicit `Return0`
    /// after every reachable path has already returned) are skipped
    /// during emit so LLVM doesn't see dead BBs without predecessors.
    pub(super) reachable: Vec<bool>,
    /// `true` at every `Op::Call` PC the scanner
    /// classified as a self-recursive call (R[A] tagged as a
    /// self-upval-loaded closure via a prior `Op::GetUpval` in the
    /// SelfMarker role). The Call lowerer emits a direct `build_call`
    /// to the current entry `FunctionValue` for these PCs.
    pub(super) self_call_pcs: Vec<bool>,
    /// `true` at every `Op::TailCall` PC the scanner
    /// classified as a self-recursive tail call. Emit: `build_call
    /// (function, args) → ret result` (Op::Call + Op::Return1 fused).
    /// Treated as a terminator: no BB successor, implies returns_one.
    pub(super) tail_call_pcs: Vec<bool>,
    /// `true` at every `Op::GetUpval` PC whose
    /// destination register is consumed as a runtime value (i.e. NOT
    /// used as the function slot of a subsequent `Op::Call`). Emitter
    /// lowers these as `luna_jit_upval_get(b as i64) -> i64`.
    /// `false` at GetUpval PCs whose role is SelfMarker — emitter
    /// writes a placeholder 0 because the matching Call rewrites
    /// straight to `build_call(function, …)` without reading R[A].
    pub(super) is_upval_value_read: Vec<bool>,
    /// The upvalue the self-recursive calls go through, if any.
    pub(super) self_upval_idx: Option<u32>,
}

impl<'a> ChunkPlan<'a> {
    pub(super) fn from_proto(proto: &'a Proto) -> Option<Self> {
        let code: &'a [Inst] = &proto.code;
        let consts: &'a [Value] = &proto.consts;
        let n = code.len();
        if n == 0 {
            return None;
        }
        // Parametric chunks need a stable fn signature. Cap at the
        // Cranelift `MAX_JIT_ARITY` (16) so the dispatcher's
        // `JitHandle` codepath stays interchangeable across backends.
        if proto.num_params as u32 > MAX_JIT_ARITY {
            return None;
        }

        let consumed_jmp = whitelist(proto, code, consts)?;

        // Pre-pass — classify each `Op::GetUpval` PC as ValueRead vs
        // SelfMarker. Mirrors Cranelift's `determine_getupval_roles`:
        // a SelfMarker is one whose destination register is consumed
        // ONLY as the function slot of a subsequent `Op::Call` (within
        // a short lookahead window, with Moves carrying the tag).
        // Anything else (arith / cmp / Return / table-access read of
        // R[A]) is a ValueRead.
        let is_upval_value_read = determine_getupval_roles(code);

        let (self_call_pcs, tail_call_pcs, self_upval_idx) =
            track_self_recursion(proto, code, &is_upval_value_read)?;

        let reachable = flow::reachable(code, &consumed_jmp);
        let returns_one = flow::returns_one(code, &reachable)?;
        let bb_starts = flow::bb_starts(code, &reachable, &consumed_jmp);

        let regs = (proto.max_stack as u32).max(proto.num_params as u32).max(1);
        if !flow::registers_in_bounds(code, &reachable, regs) {
            return None;
        }
        if !flow::reads_no_nil(code, &reachable, regs) {
            return None;
        }

        Some(ChunkPlan {
            code,
            consts,
            num_regs: regs,
            num_params: proto.num_params as u32,
            returns_one,
            bb_starts,
            consumed_jmp,
            reachable,
            self_call_pcs,
            tail_call_pcs,
            is_upval_value_read,
            self_upval_idx,
        })
    }
}

/// Pass 1: per-op whitelist gate + structural validation.
/// A comparison must be followed by a `Jmp`; mark the Jmp as
/// consumed by the condbr. The arithmetic and comparison ops
/// are lowered on integer operands only: a constant operand
/// that is not an integer refuses the function
/// (`operands::int_arith` / `int_compare`).
///
/// `Op::GetUpval` (both SelfMarker + ValueRead
/// roles, classified in a subsequent pass) and `Op::Call`
/// restricted to self-recursive shapes (validated in the
/// self_upval tracking pass below).
fn whitelist(proto: &Proto, code: &[Inst], consts: &[Value]) -> Option<Vec<bool>> {
    let n = code.len();
    let mut consumed_jmp = vec![false; n];
    for (pc, ins) in code.iter().enumerate() {
        match ins.op() {
            Op::LoadI | Op::LoadNil | Op::Move | Op::Return0 | Op::Return1 => {}
            // a jump to itself (`while true do end`, `::l:: goto l`)
            // would spin in native code, where the interpreter's
            // instruction budget and hooks never run
            Op::Jmp if jmp_target(pc, *ins) == pc => return None,
            Op::Jmp => {}
            op if is_arith(op) => {
                int_arith(*ins, consts)?;
            }
            Op::GetUpval => {
                // Upval idx must be in bounds. The self-rec /
                // value-read classification + Float-tag concerns
                // are handled by the later
                // `determine_getupval_roles` + tracking pass.
                if (ins.b() as usize) >= proto.upvals.len() {
                    return None;
                }
            }
            Op::Call => {
                // The structural gate enforced here mirrors
                // Cranelift's `Op::Call` admission: nargs / nresults
                // bounds. The (mandatory) "self-recursive"
                // classification needs the SelfMarker upval
                // tracking; done in the dedicated pass below.
                let nargs = ins.b().checked_sub(1)?;
                let nresults = ins.c().checked_sub(1)?;
                if nargs > MAX_JIT_ARITY || nresults != 1 {
                    return None;
                }
            }
            Op::TailCall => {
                // TailCall has B-1 args, no explicit nresults
                // (it returns whatever the called fn returns to OUR
                // caller). Same nargs bound as Op::Call.
                let nargs = ins.b().checked_sub(1)?;
                if nargs > MAX_JIT_ARITY {
                    return None;
                }
            }
            op if is_compare(op) => {
                int_compare(*ins, consts)?;
                let peer = code.get(pc + 1)?;
                if peer.op() != Op::Jmp {
                    return None;
                }
                consumed_jmp[pc + 1] = true;
            }
            _ => return None,
        }
    }
    Some(consumed_jmp)
}

/// Self-recursion tracking pass: walk PCs in order, maintain
/// `self_upval[reg]` (cleared on any write that doesn't carry
/// the tag), gate each `Op::Call` to the self-recursive shape.
/// Also pins `self_upval_idx` to the SelfMarker GetUpval's
/// upval slot — subsequent SelfMarker GetUpvals must read the
/// same slot, else bail. Mirrors Cranelift's S2c.C tracker.
fn track_self_recursion(
    proto: &Proto,
    code: &[Inst],
    is_upval_value_read: &[bool],
) -> Option<(Vec<bool>, Vec<bool>, Option<u32>)> {
    let n = code.len();
    let allows_self_recursion = !proto.upvals.is_empty() && proto.upvals.len() <= 4;
    let mut self_upval_idx: Option<u32> = None;
    let max_stack = (proto.max_stack as usize).max(proto.num_params as usize);
    let mut self_upval: Vec<bool> = vec![false; max_stack];
    let mut self_call_pcs: Vec<bool> = vec![false; n];
    let mut tail_call_pcs: Vec<bool> = vec![false; n];
    for (pc, ins) in code.iter().enumerate() {
        // Apply writes: any op that writes R[A] clears
        // `self_upval[A]` unless it's the SelfMarker GetUpval (which
        // sets the tag) or a Move from another self_upval-tagged
        // slot (which carries the tag).
        match ins.op() {
            Op::GetUpval => {
                if is_upval_value_read[pc] {
                    // ValueRead clears the dest tag — the loaded
                    // value is consumed as a real upvalue value,
                    // not a self-recursion call target.
                    if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                        *slot = false;
                    }
                } else {
                    // SelfMarker — must agree with any prior
                    // SelfMarker on the upval index.
                    if !allows_self_recursion {
                        return None;
                    }
                    match self_upval_idx {
                        Some(idx) if idx != ins.b() => return None,
                        Some(_) => {}
                        None => self_upval_idx = Some(ins.b()),
                    }
                    if let Some(slot) = self_upval.get_mut(ins.a() as usize) {
                        *slot = true;
                    }
                }
            }
            Op::Move => {
                let src = ins.b() as usize;
                let dst = ins.a() as usize;
                let carry = self_upval.get(src).copied().unwrap_or(false);
                if let Some(slot) = self_upval.get_mut(dst) {
                    *slot = carry;
                }
            }
            Op::Call => {
                let a = ins.a() as usize;
                if self_upval.get(a).copied().unwrap_or(false) {
                    self_call_pcs[pc] = true;
                } else {
                    // Op::Call is restricted to self-recursive
                    // shapes; non-self calls would need a general
                    // ABI marshalling shim (Cranelift bails the
                    // same way at this layer).
                    return None;
                }
                // The Call writes R[A] = result; result is the
                // self-rec return value, not a closure handle.
                if let Some(slot) = self_upval.get_mut(a) {
                    *slot = false;
                }
            }
            Op::TailCall => {
                // Same self-recursion gate as Op::Call.
                // TailCall R[A] is the function slot; it must be
                // tagged as the self-upval closure for our direct
                // `build_call` to be sound (otherwise we'd need a
                // general call ABI which is out of scope here).
                let a = ins.a() as usize;
                if self_upval.get(a).copied().unwrap_or(false) {
                    tail_call_pcs[pc] = true;
                } else {
                    return None;
                }
                // TailCall is a tail return — it does NOT write R[A]
                // (there's no subsequent op that would read the
                // result). No tag update needed for self_upval.
            }
            Op::Return1 => {
                // Returning a self_upval-tagged register would
                // ship the closure pointer back to the caller,
                // which our int-only dispatcher would misread as
                // an integer; bail.
                if self_upval.get(ins.a() as usize).copied().unwrap_or(false) {
                    return None;
                }
            }
            _ => {
                // Other ops clear the tag for any register they
                // write. For the present whitelist that's: LoadI
                // / LoadNil / the arithmetic ops — all write
                // arithmetic / immediate values that can't be a
                // closure pointer.
                let writes: &[u32] = match ins.op() {
                    Op::LoadI => &[ins.a()],
                    op if is_arith(op) => &[ins.a()],
                    // LoadNil writes R[A..=A+B]; clear them.
                    Op::LoadNil => {
                        for off in 0..=ins.b() {
                            if let Some(slot) = self_upval.get_mut((ins.a() + off) as usize) {
                                *slot = false;
                            }
                        }
                        &[]
                    }
                    _ => &[],
                };
                for &w in writes {
                    if let Some(slot) = self_upval.get_mut(w as usize) {
                        *slot = false;
                    }
                }
            }
        }
    }
    Some((self_call_pcs, tail_call_pcs, self_upval_idx))
}

/// Lua `Jmp` target: `(pc + 1) + sj`. Matches
/// `luna_jit::jit_backend::jmp_target`. Returns a `usize` — the caller
/// (ChunkPlan / compile_compute_chunk) bounds-checks against `n`
/// before using it as an index.
pub(super) fn jmp_target(pc: usize, ins: Inst) -> usize {
    (pc as i64 + 1 + ins.sj() as i64) as usize
}
