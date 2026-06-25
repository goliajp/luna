//! PUC Lua 5.3 binary-chunk translator.
//!
//! Phase LB Wave 2 (v1.3): turns a PUC 5.3 `.luac` byte stream into a
//! `Gc<Proto>` whose `code` array uses luna's 7-bit opcode encoding, so
//! the loaded Proto runs on luna's interpreter / JIT / trace JIT without
//! any per-dialect dispatch fork (Option A in
//! `.dev/rfcs/v1.3-audit-puc-luac-formats.md` §"Recommended architecture").
//!
//! Two cross-cutting decoding shifts vs luna's own body:
//!
//! 1. **6-bit opcode field** — PUC 5.3 packs an instruction as
//!    `op:6 | A:8 | C:9 | B:9` (`lopcodes.h:38-49`). luna packs
//!    `op:7 | A:8 | k:1 | B:8 | C:8`. We decode each PUC instruction
//!    into (op, A, B, C, Bx, sBx) before re-encoding into a luna `Inst`.
//!    The decoder shim (`decode_53`) is shared in spirit with the 5.1 /
//!    5.2 translators landing in subsequent waves.
//!
//! 2. **RK operands** — PUC 5.3 uses the top bit of the 9-bit B / C
//!    field (`BITRK = 1 << 8`) to flag "this is a constant-pool index"
//!    instead of "this is a register". luna stores that flag in a
//!    separate 1-bit `k` field on the instruction word, and most ops
//!    that accept RK only support it on C (Add / Sub / etc.). When a
//!    PUC op has RK on both B and C (e.g. `OP_ADD`) and the B side
//!    actually is a constant, we materialise it with a `LoadK` into the
//!    high temp register `A` (or a free temp at top of `max_stack`) and
//!    rewrite the op. **For the baseline this lands, RK-on-B is
//!    rejected with a clear error** rather than silently mistranslated —
//!    the common case (RK on C only, register on B) covers PUC's
//!    `lcode.c::luaK_exp2RK` output the vast majority of the time.
//!
//! Coverage and known gaps (full list in §"Baseline coverage" below):
//!
//! - Simple 1:1 re-encodes for all arithmetic / comparison / table /
//!   call / return / upvalue / closure ops (~30 PUC ops).
//! - `LOADBOOL` lowering: `(false,no-skip)` → `LoadFalse`; `(true,no-skip)`
//!   → `LoadTrue`; `(false,skip)` → `LFalseSkip` (PC-aligned 1:1).
//!   `(true,skip)` has no PC-preserving luna form (luna lacks
//!   `LTrueSkip`), so we reject it. This shape only fires from PUC's
//!   `lcode.c::luaK_goiftrue` for `R(A) = (cond)` patterns; uncommon
//!   in straight-line scripts.
//! - `OP_JMP A==0` → `Jmp sJ` 1:1. `OP_JMP A!=0` (close upvalues ≥
//!   R[A-1]) is rejected — supporting it requires PC remapping
//!   (`Close; Jmp` pair). Rare in practice; tracked as polish.
//! - Generic `for` (`OP_TFORCALL` / `OP_TFORLOOP`) — supported as of
//!   v2.0 PU-53 phase. 5.3 has no `OP_TFORPREP` and no TBC machinery,
//!   so the translator skips prep synthesis and emits TFORCALL/TFORLOOP
//!   directly. PUC 5.3 encodes `TFORLOOP.A` as `iter_base + 2` (since
//!   `R(A+1)` is the first call result == new ctrl, per
//!   `lvm.c:OP_TFORLOOP`), whereas luna's `Op::TForLoop` expects
//!   `A = iter_base` (reads `R(A+4)`). The translator subtracts 2 on
//!   the way through. TFORCALL.A is iter_base in both conventions, so
//!   it passes through unchanged.
//!
//! See `.dev/rfcs/v1.3-audit-puc-luac-formats.md` §"Lua 5.3 (~47 ops)"
//! and §"5.3 risks" for the full per-opcode plan; this file lands the
//! baseline subset agreed for Wave 2 LB4.

use super::super::reader::Reader;
use crate::runtime::Value;
use crate::runtime::function::{LocVar, Proto, UpvalDesc};
use crate::runtime::heap::{Gc, GcHeader, Heap, ObjTag};
use crate::vm::isa::{Inst, Op};

// ---- header constants (mirrors `lundump.h` 5.3.6) ----

const SIGNATURE: &[u8; 4] = b"\x1bLua";
const LUAC_VERSION_53: u8 = 0x53;
const LUAC_FORMAT: u8 = 0x00;
const LUAC_DATA: &[u8; 6] = b"\x19\x93\r\n\x1a\n";
const LUAC_INT: i64 = 0x5678;
const LUAC_NUM: f64 = 370.5;

// ---- 5.3 instruction layout (`lopcodes.h`) ----
//
//   bits  0.. 5  opcode (6 bits)
//   bits  6..13  A      (8 bits)
//   bits 14..22  C      (9 bits)
//   bits 23..31  B      (9 bits)
//   sign-bias for sBx:  excess MAXARG_sBx = (1 << 17 - 1) / 2 = 131071

const PUC53_SIZE_OP: u32 = 6;
const PUC53_SIZE_A: u32 = 8;
const PUC53_SIZE_B: u32 = 9;
const PUC53_SIZE_C: u32 = 9;
const PUC53_POS_A: u32 = PUC53_SIZE_OP;
const PUC53_POS_C: u32 = PUC53_POS_A + PUC53_SIZE_A;
const PUC53_POS_B: u32 = PUC53_POS_C + PUC53_SIZE_C;
const PUC53_MAX_ARG_BX: u32 = (1 << (PUC53_SIZE_B + PUC53_SIZE_C)) - 1;
const PUC53_MAX_ARG_SBX: i32 = (PUC53_MAX_ARG_BX >> 1) as i32;

/// The "this operand is a constant-pool index, not a register" bit on
/// a PUC 5.3 RK field. `BITRK = 1 << (SIZE_B - 1) = 256`.
const PUC53_BITRK: u32 = 1 << (PUC53_SIZE_B - 1);

// ---- 5.3 opcode tags (numeric values from `lopcodes.h:167-233` order) ----
//
// Order is load-bearing — PUC stores instructions by numeric opcode and
// any reordering vs `lopcodes.h` would mistranslate every chunk. We keep
// these as `const` rather than an enum to avoid round-tripping through
// repr — the file is purely a lookup table.
const OP_MOVE: u8 = 0;
const OP_LOADK: u8 = 1;
const OP_LOADKX: u8 = 2;
const OP_LOADBOOL: u8 = 3;
const OP_LOADNIL: u8 = 4;
const OP_GETUPVAL: u8 = 5;
const OP_GETTABUP: u8 = 6;
const OP_GETTABLE: u8 = 7;
const OP_SETTABUP: u8 = 8;
const OP_SETUPVAL: u8 = 9;
const OP_SETTABLE: u8 = 10;
const OP_NEWTABLE: u8 = 11;
const OP_SELF: u8 = 12;
const OP_ADD: u8 = 13;
const OP_SUB: u8 = 14;
const OP_MUL: u8 = 15;
const OP_MOD: u8 = 16;
const OP_POW: u8 = 17;
const OP_DIV: u8 = 18;
const OP_IDIV: u8 = 19;
const OP_BAND: u8 = 20;
const OP_BOR: u8 = 21;
const OP_BXOR: u8 = 22;
const OP_SHL: u8 = 23;
const OP_SHR: u8 = 24;
const OP_UNM: u8 = 25;
const OP_BNOT: u8 = 26;
const OP_NOT: u8 = 27;
const OP_LEN: u8 = 28;
const OP_CONCAT: u8 = 29;
const OP_JMP: u8 = 30;
const OP_EQ: u8 = 31;
const OP_LT: u8 = 32;
const OP_LE: u8 = 33;
const OP_TEST: u8 = 34;
const OP_TESTSET: u8 = 35;
const OP_CALL: u8 = 36;
const OP_TAILCALL: u8 = 37;
const OP_RETURN: u8 = 38;
const OP_FORLOOP: u8 = 39;
const OP_FORPREP: u8 = 40;
const OP_TFORCALL: u8 = 41;
const OP_TFORLOOP: u8 = 42;
const OP_SETLIST: u8 = 43;
const OP_CLOSURE: u8 = 44;
const OP_VARARG: u8 = 45;
const OP_EXTRAARG: u8 = 46;

const NUM_PUC53_OPS: u8 = OP_EXTRAARG + 1;

// ---- const-pool tags (`lobject.h` 5.3.6) ----

const LUA_TNIL: u8 = 0;
const LUA_TBOOLEAN: u8 = 1;
const LUA_TNUMFLT: u8 = 3; // LUA_TNUMBER | (0 << 4)
const LUA_TNUMINT: u8 = 3 | (1 << 4); // 19
const LUA_TSHRSTR: u8 = 4; // LUA_TSTRING | (0 << 4)
const LUA_TLNGSTR: u8 = 4 | (1 << 4); // 20

// ---- decoded-instruction shim ----

/// PUC 5.3 instruction decoded into its component fields. Layout-agnostic
/// — the per-opcode re-encoder below picks the iABC / iABx / iAsBx slots
/// it needs.
#[derive(Clone, Copy, Debug)]
struct Puc53Inst {
    op: u8,
    a: u32,
    b: u32,
    c: u32,
}

impl Puc53Inst {
    fn bx(self) -> u32 {
        // PUC stores Bx = B<<9 | C (or equivalently the high 18 bits).
        (self.b << PUC53_SIZE_C) | self.c
    }
    fn sbx(self) -> i32 {
        self.bx() as i32 - PUC53_MAX_ARG_SBX
    }
}

fn decode_53(word: u32) -> Puc53Inst {
    let op = (word & ((1 << PUC53_SIZE_OP) - 1)) as u8;
    let a = (word >> PUC53_POS_A) & ((1 << PUC53_SIZE_A) - 1);
    let c = (word >> PUC53_POS_C) & ((1 << PUC53_SIZE_C) - 1);
    let b = (word >> PUC53_POS_B) & ((1 << PUC53_SIZE_B) - 1);
    Puc53Inst { op, a, b, c }
}

// ---- per-dialect Reader helpers (5.3 integer/size_t are fixed widths) ----
//
// 5.3 header pins `sizeof(int) = 4`, `sizeof(size_t) = 8`,
// `sizeof(lua_Integer) = 8`, `sizeof(lua_Number) = 8` (we reject
// chunks where the header bytes don't match). Code lengths, lineinfo
// counts, constant counts, proto counts, locvar counts, upvalue counts
// are all written via PUC's `DumpInt` = native int = 4 bytes.

fn r_int(r: &mut Reader) -> Result<i32, String> {
    Ok(i32::from_le_bytes(r.take(4)?.try_into().unwrap()))
}

fn r_size(r: &mut Reader) -> Result<u64, String> {
    Ok(u64::from_le_bytes(r.take(8)?.try_into().unwrap()))
}

fn r_integer(r: &mut Reader) -> Result<i64, String> {
    Ok(i64::from_le_bytes(r.take(8)?.try_into().unwrap()))
}

fn r_number(r: &mut Reader) -> Result<f64, String> {
    Ok(f64::from_bits(u64::from_le_bytes(
        r.take(8)?.try_into().unwrap(),
    )))
}

/// 5.3 string format (`lundump.c::LoadString`):
///
/// - byte `size`:
///   - `0x00` → NULL string (source-inheritance sentinel)
///   - `0xFF` → next 8 bytes are a `size_t` length
///   - otherwise → length = `size - 1` (the `-1` is PUC's bookkeeping
///     for the implicit nul terminator inside the dump format)
fn r_string<'a>(r: &mut Reader<'a>) -> Result<Option<&'a [u8]>, String> {
    let b = r.u8()?;
    let size: u64 = if b == 0xFF { r_size(r)? } else { b as u64 };
    if size == 0 {
        return Ok(None);
    }
    let len = size
        .checked_sub(1)
        .ok_or_else(|| "bad 5.3 string size".to_string())?;
    let slice = r.take(len as usize)?;
    Ok(Some(slice))
}

// ---- header check ----

fn check_header(r: &mut Reader) -> Result<(), String> {
    let sig = r.take(4)?;
    if sig != SIGNATURE {
        return Err("bad PUC 5.3 signature".to_string());
    }
    if r.u8()? != LUAC_VERSION_53 {
        return Err("PUC 5.3 translator: version mismatch".to_string());
    }
    if r.u8()? != LUAC_FORMAT {
        return Err("PUC 5.3 translator: unsupported format byte (only 0x00)".to_string());
    }
    let data = r.take(6)?;
    if data != LUAC_DATA {
        return Err("PUC 5.3 translator: corrupted LUAC_DATA literal".to_string());
    }
    let sz_int = r.u8()?;
    if sz_int != 4 {
        return Err(format!(
            "PUC 5.3 translator: expected sizeof(int)=4, got {sz_int}"
        ));
    }
    let sz_size = r.u8()?;
    if sz_size != 8 {
        return Err(format!(
            "PUC 5.3 translator: expected sizeof(size_t)=8, got {sz_size}"
        ));
    }
    let sz_inst = r.u8()?;
    if sz_inst != 4 {
        return Err(format!(
            "PUC 5.3 translator: expected sizeof(Instruction)=4, got {sz_inst}"
        ));
    }
    let sz_int_ty = r.u8()?;
    if sz_int_ty != 8 {
        return Err(format!(
            "PUC 5.3 translator: expected sizeof(lua_Integer)=8, got {sz_int_ty}"
        ));
    }
    let sz_num = r.u8()?;
    if sz_num != 8 {
        return Err(format!(
            "PUC 5.3 translator: expected sizeof(lua_Number)=8, got {sz_num}"
        ));
    }
    let int_check = r_integer(r)?;
    if int_check != LUAC_INT {
        return Err(format!(
            "PUC 5.3 translator: endianness mismatch (LUAC_INT expected 0x5678, got 0x{int_check:x})"
        ));
    }
    let num_check = r_number(r)?;
    if num_check != LUAC_NUM {
        return Err(format!(
            "PUC 5.3 translator: float format mismatch (LUAC_NUM expected 370.5, got {num_check})"
        ));
    }
    Ok(())
}

// ---- per-opcode translator ----
//
// **Phase 4 PU Wave 2 PC remap infra** (Step 2): the per-PUC-op translation
// path used to be a pure `translate_inst(word) -> Result<Inst, String>`
// 1:1 re-encoder. Lowerings that need to emit more than one luna inst
// (RK-on-B arith via `LoadK tmp; OP a tmp c`, LOADBOOL true+skip via
// `LoadTrue; Jmp +1`, CONCAT B!=A via `Move a b; Concat a len`) cannot
// fit that signature — they shift every downstream pc, so JMP / FORLOOP /
// TFORLOOP targets must be patched after the full pc map is known and
// PUC's per-pc lineinfo must be spread across the emitted instructions.
//
// `translate_code` is the new entry point: it loops over the PUC words,
// invokes `translate_one` per op (which calls `emit_*` helpers to push
// into `code` + record both pc maps), and runs a post-pass that patches
// every recorded jump fixup against the final pc map.
//
// For now `translate_one` still returns the legacy `Inst` from a 1:1 match
// arm. Once Step 4 routes the RK-on-B arith ops through `lower_k_via_tmp`
// (and later sub-agents wire LOADBOOL+skip + CONCAT B!=A), the multi-inst
// path will start using the same `emit_more` + `jump_fixups` channels that
// the infra exposes here.

/// Output of `translate_code`: luna instruction stream plus the bidirectional
/// puc↔luna pc maps that downstream lineinfo / debug consumers need.
///
/// `puc_to_luna_pc[puc_pc]` is `Some(luna_pc)` for the **first** luna inst
/// emitted by that PUC op (or for the only one in 1:1 ops). 5.3 has no
/// dropped ops (unlike puc_54's MMBIN / VARARGPREP), so every entry is `Some`.
/// We still use `Option<u32>` so the table type matches puc_54's and the
/// helper fixup loops below stay structurally identical between dialects.
///
/// `luna_to_puc_pc[luna_pc]` is the originating PUC pc for each emitted luna
/// inst — used to spread the PUC per-pc lineinfo across multi-inst lowerings
/// (e.g. `LoadK tmp; Add a tmp c` from a single RK-on-B `ADD`).
struct Translated {
    code: Vec<Inst>,
    puc_to_luna_pc: Vec<Option<u32>>,
    luna_to_puc_pc: Vec<usize>,
    /// Worst-case extra registers needed by RK-on-B / future LOADBOOL+skip /
    /// CONCAT B!=A lowerings. Added to PUC's `max_stack` post-translation.
    max_temp_bump: u8,
}

/// One pending jump-target patch. The shape (`kind`) tells the post-pass
/// loop how to encode the resolved distance back into the placeholder inst.
#[derive(Clone, Copy, Debug)]
enum FixupKind {
    /// `Op::Jmp` placeholder — encode as sJ (signed offset from next pc).
    /// Used by `OP_JMP` (and by Step 3+ LOADBOOL+skip lowering).
    Jmp,
    /// `Op::ForPrep A Bx` placeholder — encode as positive Bx forward offset
    /// (PUC's `OP_FORPREP A sBx` uses `sBx >= 0` forward jump). luna ForPrep
    /// dispatcher matches PUC's `pc += sBx` semantics directly, so no off-by-one.
    ForPrep,
    /// `Op::ForLoop A Bx` placeholder — encode as positive Bx backward offset
    /// (`PUC FORLOOP A sBx` uses `sBx < 0`; luna `Bx = -sBx`).
    ForLoop,
    /// `Op::TForLoop A Bx` placeholder — same backward-Bx encoding as ForLoop.
    TForLoop,
}

#[derive(Clone, Copy, Debug)]
struct Fixup {
    luna_pc: usize,
    /// PUC pc the original instruction targeted (its first byte's pc — the
    /// post-pass remaps via `puc_to_luna_pc[target_puc_pc]`).
    target_puc_pc: i64,
    kind: FixupKind,
    /// Original `A` operand for the iABx ops (FORPREP / FORLOOP / TFORLOOP) —
    /// they encode A alongside Bx so we need it at patch time.
    a: u32,
}

/// Translate a full PUC 5.3 instruction stream into luna's encoding,
/// recording the puc↔luna pc maps for downstream lineinfo + jump fixup
/// consumers. Returns `Err(...)` for opcodes / shapes the baseline doesn't
/// cover; the caller surfaces the error verbatim to the embedder.
fn translate_code(puc_code: &[u32]) -> Result<Translated, String> {
    let mut code: Vec<Inst> = Vec::with_capacity(puc_code.len());
    let mut puc_to_luna_pc: Vec<Option<u32>> = Vec::with_capacity(puc_code.len());
    let mut luna_to_puc_pc: Vec<usize> = Vec::with_capacity(puc_code.len());
    let mut max_temp_bump: u8 = 0;

    // Stash pending jump fixups. Populated by `translate_one` for every
    // PC-relative op (`JMP`, `FORPREP`, `FORLOOP`, `TFORLOOP`). The post-pass
    // walks the list and rewrites each placeholder against the final pc map,
    // so multi-inst lowerings (Step 4+ RK-on-B / LOADBOOL+skip / CONCAT B!=A)
    // that shift downstream pcs stay correct without per-op accounting.
    let mut jump_fixups: Vec<Fixup> = Vec::new();

    for (puc_pc, &word) in puc_code.iter().enumerate() {
        translate_one(
            puc_pc,
            word,
            &mut code,
            &mut puc_to_luna_pc,
            &mut luna_to_puc_pc,
            &mut max_temp_bump,
            &mut jump_fixups,
        )
        .map_err(|e| format!("{e} (pc={puc_pc})"))?;
    }

    // Resolve every pending fixup. For each one, look up the target PUC pc in
    // `puc_to_luna_pc` to get the destination luna pc, then encode the
    // distance back into the placeholder instruction per its `FixupKind`.
    for fx in jump_fixups {
        let target_luna_pc = resolve_target(fx.target_puc_pc, &puc_to_luna_pc, code.len())?;
        let next_pc = fx.luna_pc as i64 + 1;
        let delta = target_luna_pc - next_pc;
        let inst = match fx.kind {
            FixupKind::Jmp => {
                // sJ encoding: signed offset from next pc.
                Inst::isj(Op::Jmp, delta as i32)
            }
            FixupKind::ForPrep => {
                // Forward jump — `pc += Bx` per luna ForPrep dispatcher.
                if delta < 0 {
                    return Err(format!(
                        "PUC 5.3 FORPREP at luna pc {} resolved to backward delta {} \
                         (FORPREP must jump forward)",
                        fx.luna_pc, delta
                    ));
                }
                let bx = delta as u32;
                if bx > crate::vm::isa::MAX_BX {
                    return Err(format!(
                        "PUC 5.3 FORPREP at luna pc {} forward distance {bx} \
                         out of luna Bx range",
                        fx.luna_pc
                    ));
                }
                Inst::iabx(Op::ForPrep, fx.a, bx)
            }
            FixupKind::ForLoop => {
                // Backward jump — luna `Bx = -delta` (positive back-distance).
                if delta > 0 {
                    return Err(format!(
                        "PUC 5.3 FORLOOP at luna pc {} resolved to forward delta {} \
                         (FORLOOP must jump backward)",
                        fx.luna_pc, delta
                    ));
                }
                let back = (-delta) as u32;
                if back > crate::vm::isa::MAX_BX {
                    return Err(format!(
                        "PUC 5.3 FORLOOP at luna pc {} back-distance {back} \
                         out of luna Bx range",
                        fx.luna_pc
                    ));
                }
                Inst::iabx(Op::ForLoop, fx.a, back)
            }
            FixupKind::TForLoop => {
                if delta > 0 {
                    return Err(format!(
                        "PUC 5.3 TFORLOOP at luna pc {} resolved to forward delta {} \
                         (TFORLOOP must jump backward)",
                        fx.luna_pc, delta
                    ));
                }
                let back = (-delta) as u32;
                if back > crate::vm::isa::MAX_BX {
                    return Err(format!(
                        "PUC 5.3 TFORLOOP at luna pc {} back-distance {back} \
                         out of luna Bx range",
                        fx.luna_pc
                    ));
                }
                Inst::iabx(Op::TForLoop, fx.a, back)
            }
        };
        code[fx.luna_pc] = inst;
    }

    Ok(Translated {
        code,
        puc_to_luna_pc,
        luna_to_puc_pc,
        max_temp_bump,
    })
}

/// Map a PUC 5.3 arith opcode to its luna `Op` counterpart, or `None` for
/// non-arith ops. The router in `translate_one` uses this to decide whether
/// to call `lower_k_via_tmp` for the RK-on-B shape; non-matching opcodes fall
/// straight through to `translate_inst`.
fn arith_op_for(puc_op: u8) -> Option<Op> {
    Some(match puc_op {
        OP_ADD => Op::Add,
        OP_SUB => Op::Sub,
        OP_MUL => Op::Mul,
        OP_MOD => Op::Mod,
        OP_POW => Op::Pow,
        OP_DIV => Op::Div,
        OP_IDIV => Op::IDiv,
        OP_BAND => Op::BAnd,
        OP_BOR => Op::BOr,
        OP_BXOR => Op::BXor,
        OP_SHL => Op::Shl,
        OP_SHR => Op::Shr,
        _ => return None,
    })
}

/// Resolve a PUC target pc to the corresponding luna pc through the
/// `puc_to_luna_pc` map. 5.3 has no dropped ops today so the inner loop
/// terminates on its first iteration; the structure matches puc_54.rs so
/// a future 5.3 drop shape needs no rewrite.
fn resolve_target(
    target_puc_pc: i64,
    puc_to_luna_pc: &[Option<u32>],
    code_len: usize,
) -> Result<i64, String> {
    if target_puc_pc < 0 {
        return Err(format!("PUC 5.3 jump targets negative pc {target_puc_pc}"));
    }
    if target_puc_pc as usize >= puc_to_luna_pc.len() {
        // Past-the-end: target the synthetic end-of-code pc.
        return Ok(code_len as i64);
    }
    let mut t = target_puc_pc as usize;
    loop {
        if t >= puc_to_luna_pc.len() {
            return Ok(code_len as i64);
        }
        if let Some(p) = puc_to_luna_pc[t] {
            return Ok(p as i64);
        }
        t += 1;
    }
}

/// Per-PUC-op translation step. Pushes one or more luna `Inst`s into `code`
/// and records the pc mapping (`puc_to_luna_pc[puc_pc] = Some(first_luna_pc)`;
/// each emitted inst pushes a `luna_to_puc_pc[luna_pc] = puc_pc` entry).
///
/// PC-relative ops (`JMP`, `FORPREP`, `FORLOOP`, `TFORLOOP`) take the
/// fixup channel: a placeholder is emitted now and the real offset is
/// patched in by `translate_code` once the full pc map is known. Multi-inst
/// lowerings (Step 4+ RK-on-B / LOADBOOL+skip / CONCAT B!=A) shift downstream
/// pcs, so the per-op `i.sbx()` computed at decode time is no longer the
/// luna offset — only the fixup loop sees the final layout.
///
/// 5.3 has no dropped-op shape, so every call appends at least one entry
/// to both maps. Everything that isn't a PC-relative op delegates to
/// `translate_inst` for the 1:1 fast path.
#[allow(clippy::too_many_arguments)]
fn translate_one(
    puc_pc: usize,
    word: u32,
    code: &mut Vec<Inst>,
    puc_to_luna_pc: &mut Vec<Option<u32>>,
    luna_to_puc_pc: &mut Vec<usize>,
    max_temp_bump: &mut u8,
    jump_fixups: &mut Vec<Fixup>,
) -> Result<(), String> {
    let i = decode_53(word);
    if i.op >= NUM_PUC53_OPS {
        return Err(format!(
            "PUC 5.3 translator: unknown opcode {} (max {})",
            i.op,
            NUM_PUC53_OPS - 1
        ));
    }

    // Decode RK bits on the 9-bit B / C operands so the RK-on-B arith router
    // below can peek before delegating to `translate_inst`.
    let b_is_k = i.b & PUC53_BITRK != 0;
    let c_is_k = i.c & PUC53_BITRK != 0;
    let b_idx = i.b & (PUC53_BITRK - 1);
    let c_idx = i.c & (PUC53_BITRK - 1);

    // --- RK-on-B arith family: route through `lower_k_via_tmp` ---
    //
    // PUC 5.3's `OP_ADD` (and friends) lets either operand be RK; luna's `Add`
    // only accepts the k flag on C. When B is the constant we materialize it
    // via `LoadK tmp; OP a tmp c` — pure helper call, no new local logic. C
    // can still be RK (passed through as the helper's `c_is_k` arg).
    //
    // The B-is-register path keeps falling through to `translate_inst`'s
    // existing arith arm, which is the hot path in stock chunks (PUC's
    // `luaK_exp2RK` prefers the C slot for the constant).
    if let Some(luna_op) = arith_op_for(i.op) {
        if b_is_k {
            // Allocate a temp register above PUC's own usage. Same policy as
            // puc_54.rs's I-imm lowering: `max(a, c) + 1` keeps the tmp slot
            // distinct from both source registers (the arith reads `b/c` and
            // writes `a`, so `tmp` must dodge `a` *and* the live C register).
            let tmp = i.a.max(c_idx) + 1;
            let pair =
                super::lower_k_via_tmp(luna_op, i.a, b_idx, c_idx, c_is_k, tmp, max_temp_bump)?;
            let first_luna_pc = code.len() as u32;
            puc_to_luna_pc.push(Some(first_luna_pc));
            luna_to_puc_pc.push(puc_pc);
            code.push(pair[0]);
            luna_to_puc_pc.push(puc_pc);
            code.push(pair[1]);
            return Ok(());
        }
        // B is a register → 1:1 path; let `translate_inst` handle bounds.
    }

    // --- PC-relative ops: emit placeholder + push fixup ---
    match i.op {
        OP_JMP => {
            if i.a != 0 {
                return Err(
                    "PUC 5.3 translator: OP_JMP with close-upvalues hint (A != 0) not yet \
                     supported (planned: emit `Close (A-1); Jmp sBx` with PC remap)"
                        .to_string(),
                );
            }
            // PUC sBx is the signed offset from next pc; the target PUC pc is
            // therefore `puc_pc + 1 + sBx`. The post-pass remaps that to the
            // matching luna pc.
            let target = puc_pc as i64 + 1 + i.sbx() as i64;
            let luna_pc = code.len();
            puc_to_luna_pc.push(Some(luna_pc as u32));
            luna_to_puc_pc.push(puc_pc);
            code.push(Inst::isj(Op::Jmp, 0)); // placeholder
            jump_fixups.push(Fixup {
                luna_pc,
                target_puc_pc: target,
                kind: FixupKind::Jmp,
                a: 0,
            });
            return Ok(());
        }
        OP_FORPREP => {
            // PUC `OP_FORPREP A sBx`: pc += sBx (forward, positive sBx).
            let sbx = i.sbx();
            let target = puc_pc as i64 + 1 + sbx as i64;
            let luna_pc = code.len();
            puc_to_luna_pc.push(Some(luna_pc as u32));
            luna_to_puc_pc.push(puc_pc);
            code.push(Inst::iabx(Op::ForPrep, i.a, 0)); // placeholder Bx
            jump_fixups.push(Fixup {
                luna_pc,
                target_puc_pc: target,
                kind: FixupKind::ForPrep,
                a: i.a,
            });
            return Ok(());
        }
        OP_FORLOOP => {
            // PUC `OP_FORLOOP A sBx`: pc += sBx (backward, negative sBx).
            let sbx = i.sbx();
            let target = puc_pc as i64 + 1 + sbx as i64;
            let luna_pc = code.len();
            puc_to_luna_pc.push(Some(luna_pc as u32));
            luna_to_puc_pc.push(puc_pc);
            code.push(Inst::iabx(Op::ForLoop, i.a, 0));
            jump_fixups.push(Fixup {
                luna_pc,
                target_puc_pc: target,
                kind: FixupKind::ForLoop,
                a: i.a,
            });
            return Ok(());
        }
        OP_TFORLOOP => {
            // PUC `OP_TFORLOOP A sBx`: if R(A+1) ~= nil { R(A) = R(A+1); pc += sBx }.
            // luna `Op::TForLoop iABx A Bx` expects `A = iter_base` (PUC's A is
            // `iter_base + 2`) and a positive backward Bx (`-sBx`). Mirror the
            // existing 1:1 translation: convert A here and let the fixup loop
            // resolve the absolute target through the pc map.
            let sbx = i.sbx();
            if sbx > 0 {
                return Err(format!(
                    "PUC 5.3 translator: TFORLOOP sBx {sbx} > 0 (expected backward jump)"
                ));
            }
            let iter_base = i.a.checked_sub(2).ok_or_else(|| {
                format!(
                    "PUC 5.3 translator: TFORLOOP.A={} < 2 (cannot convert to luna iter_base)",
                    i.a
                )
            })?;
            let target = puc_pc as i64 + 1 + sbx as i64;
            let luna_pc = code.len();
            puc_to_luna_pc.push(Some(luna_pc as u32));
            luna_to_puc_pc.push(puc_pc);
            code.push(Inst::iabx(Op::TForLoop, iter_base, 0));
            jump_fixups.push(Fixup {
                luna_pc,
                target_puc_pc: target,
                kind: FixupKind::TForLoop,
                a: iter_base,
            });
            return Ok(());
        }
        _ => {}
    }

    // --- everything else: 1:1 via `translate_inst` ---
    let inst = translate_inst(word)?;
    puc_to_luna_pc.push(Some(code.len() as u32));
    luna_to_puc_pc.push(puc_pc);
    code.push(inst);
    Ok(())
}

/// Translate one PUC 5.3 instruction word into a luna `Inst`. Returns
/// `Err(...)` for opcodes / shapes the baseline doesn't cover; the caller
/// surfaces the error verbatim to the embedder.
fn translate_inst(word: u32) -> Result<Inst, String> {
    let i = decode_53(word);
    if i.op >= NUM_PUC53_OPS {
        return Err(format!(
            "PUC 5.3 translator: unknown opcode {} (max {})",
            i.op,
            NUM_PUC53_OPS - 1
        ));
    }
    // Decode RK bit on the 9-bit B / C operands. PUC's `BITRK` (bit 8)
    // flags "constant index"; mask it off and report the kind to the
    // re-encoder. luna's `k` is a single flag and lives on the
    // instruction word, not on the operand, so we lose nothing by
    // splitting them here.
    let b_is_k = i.b & PUC53_BITRK != 0;
    let c_is_k = i.c & PUC53_BITRK != 0;
    let b_idx = i.b & (PUC53_BITRK - 1);
    let c_idx = i.c & (PUC53_BITRK - 1);

    /// Bounds-check an operand against luna's narrower 8-bit B / C field.
    /// PUC 5.3 has 9-bit B/C (max 511 incl. RK bit, 255 once stripped)
    /// so post-mask values up to 255 fit; chunks that index beyond that
    /// must come from a non-stock compiler.
    fn fit_b(v: u32, name: &str) -> Result<u32, String> {
        if v > 0xFF {
            Err(format!(
                "PUC 5.3 translator: {name} operand {v} > 255 — out of luna's 8-bit field"
            ))
        } else {
            Ok(v)
        }
    }
    /// luna ops like `Add` / `Sub` accept RK only on C. If a chunk has
    /// the constant on B, we'd need to materialise it via `LoadK`
    /// (`R[tmp] := K[B_idx]`), then emit the op with B = tmp. That
    /// requires a free temp register + a post-pass `max_stack` bump +
    /// PC re-mapping, which baseline doesn't do.
    fn reject_b_k(opname: &str, b_is_k: bool) -> Result<(), String> {
        if b_is_k {
            Err(format!(
                "PUC 5.3 translator: {opname} with RK on B not yet supported \
                 (baseline accepts only RK on C; emit a `LoadK tmp; OP A tmp B` \
                 sequence is the planned polish)"
            ))
        } else {
            Ok(())
        }
    }

    Ok(match i.op {
        // ---- simple iABC re-encodes, no RK ----
        OP_MOVE => Inst::iabc(Op::Move, i.a, fit_b(b_idx, "MOVE.B")?, 0, false),
        OP_LOADNIL => {
            // 5.2+ shape: `R[A..A+B] := nil` — same as luna's `LoadNil`.
            Inst::iabc(Op::LoadNil, i.a, fit_b(b_idx, "LOADNIL.B")?, 0, false)
        }
        OP_GETUPVAL => Inst::iabc(Op::GetUpval, i.a, fit_b(b_idx, "GETUPVAL.B")?, 0, false),
        OP_SETUPVAL => Inst::iabc(Op::SetUpval, i.a, fit_b(b_idx, "SETUPVAL.B")?, 0, false),
        OP_NEWTABLE => {
            // 5.3 stores hints as `luaO_int2fb` floating-byte encoding;
            // for the baseline we pass through verbatim — luna treats
            // hint operands as advisory (`NewTable` reads B/C as sizes).
            // The fb-int re-encode is a low-impact polish.
            Inst::iabc(
                Op::NewTable,
                i.a,
                fit_b(b_idx, "NEWTABLE.B")?,
                fit_b(c_idx, "NEWTABLE.C")?,
                false,
            )
        }
        OP_NOT => Inst::iabc(Op::Not, i.a, fit_b(b_idx, "NOT.B")?, 0, false),
        OP_LEN => Inst::iabc(Op::Len, i.a, fit_b(b_idx, "LEN.B")?, 0, false),
        OP_UNM => Inst::iabc(Op::Unm, i.a, fit_b(b_idx, "UNM.B")?, 0, false),
        OP_BNOT => Inst::iabc(Op::BNot, i.a, fit_b(b_idx, "BNOT.B")?, 0, false),

        // ---- iABx loads ----
        OP_LOADK => Inst::iabx(Op::LoadK, i.a, i.bx()),
        OP_LOADKX => Inst::iabx(Op::LoadKx, i.a, 0),

        // ---- LOADBOOL → LoadFalse/LoadTrue/LFalseSkip (1:1 PC) ----
        OP_LOADBOOL => match (b_idx, c_idx) {
            (0, 0) => Inst::iabc(Op::LoadFalse, i.a, 0, 0, false),
            (0, _) => Inst::iabc(Op::LFalseSkip, i.a, 0, 0, false),
            (_, 0) => Inst::iabc(Op::LoadTrue, i.a, 0, 0, false),
            (_, _) => {
                return Err("PUC 5.3 translator: LOADBOOL true+skip not yet supported \
                     (luna has no LTrueSkip; planned polish is to emit \
                     `LoadTrue; Jmp +1` with PC remap)"
                    .to_string());
            }
        },

        // ---- table reads/writes ----
        // PUC GETTABUP / GETTABLE / SETTABUP / SETTABLE accept RK on the
        // key (C for read, B for SETTABLE/SETTABUP); luna's GetTabUp
        // takes a constant-pool string key only (it asserts `K[C]:string`).
        // For the baseline, we accept RK on the key when it points at the
        // constant pool (the only stock-compiler shape — `t.x` /
        // `_ENV.x`) and reject the register-key case (`t[r]` against an
        // upvalue table — uncommon).
        OP_GETTABUP => {
            if !c_is_k {
                return Err(
                    "PUC 5.3 translator: GETTABUP with register key — not yet supported \
                     (only constant-string keys land in baseline)"
                        .to_string(),
                );
            }
            Inst::iabc(
                Op::GetTabUp,
                i.a,
                fit_b(b_idx, "GETTABUP.B")?,
                fit_b(c_idx, "GETTABUP.C")?,
                false,
            )
        }
        OP_GETTABLE => {
            if c_is_k {
                // Treat as GetField (constant-string key on a register
                // table) — luna's GetField matches this shape 1:1 and
                // skips the runtime type check on the key.
                Inst::iabc(
                    Op::GetField,
                    i.a,
                    fit_b(b_idx, "GETTABLE.B")?,
                    fit_b(c_idx, "GETTABLE.C")?,
                    false,
                )
            } else {
                Inst::iabc(
                    Op::GetTable,
                    i.a,
                    fit_b(b_idx, "GETTABLE.B")?,
                    fit_b(c_idx, "GETTABLE.C")?,
                    false,
                )
            }
        }
        OP_SETTABUP => {
            // SETTABUP A B C: UpValue[A][RK(B)] := RK(C)
            // luna SetTabUp: Upvalues[A][K[B]:string] := R[C]/K[C]
            if !b_is_k {
                return Err(
                    "PUC 5.3 translator: SETTABUP with register key — not yet supported"
                        .to_string(),
                );
            }
            Inst::iabc(
                Op::SetTabUp,
                i.a,
                fit_b(b_idx, "SETTABUP.B")?,
                fit_b(c_idx, "SETTABUP.C")?,
                c_is_k,
            )
        }
        OP_SETTABLE => {
            // SETTABLE A B C: R[A][RK(B)] := RK(C)
            // luna SetTable: R[A][R[B]] := R[C]/K[C]; SetField for str key
            if b_is_k {
                Inst::iabc(
                    Op::SetField,
                    i.a,
                    fit_b(b_idx, "SETTABLE.B")?,
                    fit_b(c_idx, "SETTABLE.C")?,
                    c_is_k,
                )
            } else {
                Inst::iabc(
                    Op::SetTable,
                    i.a,
                    fit_b(b_idx, "SETTABLE.B")?,
                    fit_b(c_idx, "SETTABLE.C")?,
                    c_is_k,
                )
            }
        }
        OP_SELF => {
            if !c_is_k {
                return Err(
                    "PUC 5.3 translator: SELF with register key — not yet supported".to_string(),
                );
            }
            Inst::iabc(
                Op::SelfOp,
                i.a,
                fit_b(b_idx, "SELF.B")?,
                fit_b(c_idx, "SELF.C")?,
                false,
            )
        }

        // ---- arithmetic / bitwise (RK on C; reject RK on B) ----
        OP_ADD => arith(
            Op::Add,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "ADD",
            reject_b_k,
        )?,
        OP_SUB => arith(
            Op::Sub,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "SUB",
            reject_b_k,
        )?,
        OP_MUL => arith(
            Op::Mul,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "MUL",
            reject_b_k,
        )?,
        OP_MOD => arith(
            Op::Mod,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "MOD",
            reject_b_k,
        )?,
        OP_POW => arith(
            Op::Pow,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "POW",
            reject_b_k,
        )?,
        OP_DIV => arith(
            Op::Div,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "DIV",
            reject_b_k,
        )?,
        OP_IDIV => arith(
            Op::IDiv,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "IDIV",
            reject_b_k,
        )?,
        OP_BAND => arith(
            Op::BAnd,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "BAND",
            reject_b_k,
        )?,
        OP_BOR => arith(
            Op::BOr,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "BOR",
            reject_b_k,
        )?,
        OP_BXOR => arith(
            Op::BXor,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "BXOR",
            reject_b_k,
        )?,
        OP_SHL => arith(
            Op::Shl,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "SHL",
            reject_b_k,
        )?,
        OP_SHR => arith(
            Op::Shr,
            i.a,
            b_idx,
            b_is_k,
            c_idx,
            c_is_k,
            "SHR",
            reject_b_k,
        )?,

        OP_CONCAT => {
            // 5.3 CONCAT A B C: R[A] := R[B]..R[B+1]..…..R[C]
            // luna CONCAT A B: R[A] := R[A]..…..R[A+B-1]
            // The two shapes differ — 5.3 uses (B,C) range; luna uses
            // (A,B-len) range. They match when B == A and C - A + 1 == B_luna.
            if b_idx != i.a {
                return Err("PUC 5.3 translator: CONCAT with B != A not yet supported \
                     (would require a MOVE pre-pass)"
                    .to_string());
            }
            let len = c_idx
                .checked_sub(b_idx)
                .ok_or("PUC 5.3 translator: CONCAT C < B")?
                + 1;
            Inst::iabc(Op::Concat, i.a, fit_b(len, "CONCAT len")?, 0, false)
        }

        // ---- jump ----
        OP_JMP => {
            if i.a != 0 {
                return Err(
                    "PUC 5.3 translator: OP_JMP with close-upvalues hint (A != 0) not yet \
                     supported (planned: emit `Close (A-1); Jmp sBx` with PC remap)"
                        .to_string(),
                );
            }
            Inst::isj(Op::Jmp, i.sbx())
        }

        // ---- comparisons. PUC: if (RK(B) <op> RK(C)) != A then pc++ ----
        // luna: same shape on Eq/Lt/Le, k carries the comparison sense.
        OP_EQ => cmp(Op::Eq, i.a, b_idx, b_is_k, c_idx, c_is_k, "EQ")?,
        OP_LT => cmp(Op::Lt, i.a, b_idx, b_is_k, c_idx, c_is_k, "LT")?,
        OP_LE => cmp(Op::Le, i.a, b_idx, b_is_k, c_idx, c_is_k, "LE")?,

        OP_TEST => {
            // PUC TEST A C: if not (R(A) <=> C) then pc++. C ∈ {0,1}.
            // luna Test  A k: if (not R[A]) == k then pc++
            // These are equivalent with k = (C != 0).
            Inst::iabc(Op::Test, i.a, 0, 0, c_idx != 0)
        }
        OP_TESTSET => Inst::iabc(Op::TestSet, i.a, fit_b(b_idx, "TESTSET.B")?, 0, c_idx != 0),

        // ---- calls / returns ----
        OP_CALL => Inst::iabc(
            Op::Call,
            i.a,
            fit_b(b_idx, "CALL.B")?,
            fit_b(c_idx, "CALL.C")?,
            false,
        ),
        OP_TAILCALL => Inst::iabc(
            Op::TailCall,
            i.a,
            fit_b(b_idx, "TAILCALL.B")?,
            fit_b(c_idx, "TAILCALL.C")?,
            false,
        ),
        OP_RETURN => Inst::iabc(Op::Return, i.a, fit_b(b_idx, "RETURN.B")?, 0, false),

        // ---- numeric for ----
        // PUC OP_FORPREP A sBx: R[A] -= R[A+2]; pc += sBx
        // PUC OP_FORLOOP A sBx: step; if cond then { pc += sBx; R[A+3] = R[A] }
        // luna ForPrep is iABx Bx = forward skip distance == sBx (no
        // conversion needed — pre53 interp handles the `bx - 1` landing).
        // luna ForLoop is iABx Bx = absolute backward distance = -sBx.
        OP_FORPREP => {
            let sbx = i.sbx();
            if !(0..=crate::vm::isa::MAX_BX as i32).contains(&sbx) {
                return Err(format!(
                    "PUC 5.3 translator: FORPREP sBx {sbx} out of luna Bx range"
                ));
            }
            Inst::iabx(Op::ForPrep, i.a, sbx as u32)
        }
        OP_FORLOOP => {
            let sbx = i.sbx();
            if sbx > 0 {
                return Err(format!(
                    "PUC 5.3 translator: FORLOOP sBx {sbx} > 0 (expected backward jump)"
                ));
            }
            let back = (-sbx) as u32;
            if back > crate::vm::isa::MAX_BX {
                return Err(format!(
                    "PUC 5.3 translator: FORLOOP back-distance {back} out of luna Bx range"
                ));
            }
            Inst::iabx(Op::ForLoop, i.a, back)
        }

        OP_TFORCALL => {
            // PUC 5.3 TFORCALL A C: R(A+3)..R(A+2+C) := R(A)(R(A+1), R(A+2)).
            // luna Op::TForCall iABC A 0 C: dispatcher copies iter→R(A+4),
            // state→R(A+5), ctrl→R(A+6), calls, writes C results starting
            // at R(A+4). A is iter_base in both conventions — passes through.
            Inst::iabc(Op::TForCall, i.a, 0, fit_b(c_idx, "TFORCALL.C")?, false)
        }
        OP_TFORLOOP => {
            // PUC 5.3 TFORLOOP A sBx: if R(A+1) ~= nil then R(A) = R(A+1); pc += sBx.
            // Here A is `iter_base + 2` (so R(A+1) = R(iter_base + 3) = first call
            // result, R(A) = R(iter_base + 2) = ctrl slot). sBx is signed and
            // negative for the back-jump.
            //
            // luna Op::TForLoop iABx A Bx: reads R(A+4) for ctrl, expects
            // A = iter_base, Bx = positive back-distance. Convert by
            // subtracting 2 from A and negating sBx.
            let sbx = i.sbx();
            if sbx > 0 {
                return Err(format!(
                    "PUC 5.3 translator: TFORLOOP sBx {sbx} > 0 (expected backward jump)"
                ));
            }
            let back = (-sbx) as u32;
            if back > crate::vm::isa::MAX_BX {
                return Err(format!(
                    "PUC 5.3 translator: TFORLOOP back-distance {back} out of luna Bx range"
                ));
            }
            let iter_base = i.a.checked_sub(2).ok_or_else(|| {
                format!(
                    "PUC 5.3 translator: TFORLOOP.A={} < 2 (cannot convert to luna iter_base)",
                    i.a
                )
            })?;
            Inst::iabx(Op::TForLoop, iter_base, back)
        }

        OP_SETLIST => {
            if c_idx == 0 {
                return Err(
                    "PUC 5.3 translator: SETLIST with C == 0 (EXTRAARG block-index) \
                     not yet supported"
                        .to_string(),
                );
            }
            Inst::iabc(
                Op::SetList,
                i.a,
                fit_b(b_idx, "SETLIST.B")?,
                fit_b(c_idx, "SETLIST.C")?,
                false,
            )
        }

        OP_CLOSURE => Inst::iabx(Op::Closure, i.a, i.bx()),

        OP_VARARG => Inst::iabc(Op::Vararg, i.a, fit_b(b_idx, "VARARG.B")?, 0, false),

        OP_EXTRAARG => Inst::iax(Op::ExtraArg, i.bx()),

        _ => unreachable!("opcode-range guard above ruled this out"),
    })
}

#[allow(clippy::too_many_arguments)]
fn arith(
    op: Op,
    a: u32,
    b: u32,
    b_is_k: bool,
    c: u32,
    c_is_k: bool,
    name: &str,
    reject_b_k: fn(&str, bool) -> Result<(), String>,
) -> Result<Inst, String> {
    reject_b_k(name, b_is_k)?;
    if b > 0xFF {
        return Err(format!("PUC 5.3 translator: {name}.B operand {b} > 255"));
    }
    if c > 0xFF {
        return Err(format!("PUC 5.3 translator: {name}.C operand {c} > 255"));
    }
    Ok(Inst::iabc(op, a, b, c, c_is_k))
}

fn cmp(
    op: Op,
    a: u32,
    b: u32,
    b_is_k: bool,
    c: u32,
    c_is_k: bool,
    name: &str,
) -> Result<Inst, String> {
    // **5.3 → luna field shuffle** for comparisons. PUC 5.3
    // `OP_EQ A B C` means `if (RK(B) <op> RK(C)) != A then pc++` — so
    // A is the test-sense, B/C are RK operands. luna `Eq A B (k)`
    // means `if (R[A] <op> R[B]) != k then pc++` — A is the LHS
    // register, B is the RHS register, `k` carries the test-sense.
    // Translation:
    //   luna A = PUC B (LHS register)
    //   luna B = PUC C (RHS register)
    //   luna k = PUC A (test-sense; A is 0 or 1 in stock chunks)
    //
    // PUC `OP_EQ` is the only comparison that has a luna constant-key
    // shortcut (`Op::EqK A B (k)` fetches K[B] — but it only handles
    // `EqK`, not Lt/Le). For the baseline, RK-on-either-side reduces to
    // "must be register" for Lt/Le and "must be register OR Eq+rhs-K"
    // for Eq. To keep this file small, **baseline only accepts plain
    // register operands** (no RK on either side). A polish pass adds
    // EqK lowering + per-op constant materialisation; the rejection
    // path here yields a clear "not yet supported" error rather than a
    // silent miscompile.
    if b_is_k || c_is_k {
        return Err(format!(
            "PUC 5.3 translator: {name} with RK operand not yet supported \
             (baseline accepts only register operands)"
        ));
    }
    if b > 0xFF || c > 0xFF {
        return Err(format!(
            "PUC 5.3 translator: {name} B/C operand out of luna 8-bit range"
        ));
    }
    let k = a != 0;
    Ok(Inst::iabc(op, b, c, 0, k))
}

// ---- constant pool ----

fn r_const(r: &mut Reader, heap: &mut Heap) -> Result<Value, String> {
    Ok(match r.u8()? {
        LUA_TNIL => Value::Nil,
        LUA_TBOOLEAN => Value::Bool(r.u8()? != 0),
        LUA_TNUMFLT => Value::Float(r_number(r)?),
        LUA_TNUMINT => Value::Int(r_integer(r)?),
        LUA_TSHRSTR | LUA_TLNGSTR => {
            // 5.3 strings inside the const pool always have a length
            // (never NULL — that's the per-Proto source slot only).
            match r_string(r)? {
                Some(b) => Value::Str(heap.intern(b)),
                None => return Err("PUC 5.3 translator: NULL string in const pool".to_string()),
            }
        }
        t => return Err(format!("PUC 5.3 translator: bad constant tag {t}")),
    })
}

// ---- Proto recursion ----

fn r_proto(
    r: &mut Reader,
    heap: &mut Heap,
    parent_source: Option<Gc<crate::runtime::LuaStr>>,
) -> Result<Gc<Proto>, String> {
    // 5.3 LoadFunction order:
    //   source string (may be NULL → inherit parent)
    //   linedefined (int)
    //   lastlinedefined (int)
    //   numparams (byte)
    //   is_vararg (byte)
    //   maxstacksize (byte)
    //   code (int n + n*Instruction)
    //   constants
    //   upvalues (just instack+idx; names come from debug section)
    //   protos (recursive)
    //   debug (lineinfo + locvars + upvalue names)
    let source = match r_string(r)? {
        Some(b) => heap.intern(b),
        None => parent_source.unwrap_or_else(|| heap.intern(b"")),
    };
    let line_defined = r_int(r)? as u32;
    let last_line_defined = r_int(r)? as u32;
    let num_params = r.u8()?;
    let is_vararg = r.u8()? != 0;
    let max_stack_puc = r.u8()?;

    // ---- code ----
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative code length".to_string());
    }
    let n = n as usize;
    // Read the raw PUC words first; `translate_code` needs the full stream so
    // post-translation jump-fixups can resolve targets against the final pc
    // map (multi-inst lowerings — RK-on-B / LOADBOOL+skip / CONCAT B!=A —
    // shift downstream pcs in ways the per-instruction loop can't see).
    let mut puc_words: Vec<u32> = Vec::with_capacity(n);
    for _ in 0..n {
        puc_words.push(u32::from_le_bytes(r.take(4)?.try_into().unwrap()));
    }
    let translated = translate_code(&puc_words)?;

    // ---- constants ----
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative const count".to_string());
    }
    let mut consts = Vec::with_capacity(n as usize);
    for _ in 0..n {
        consts.push(r_const(r, heap)?);
    }

    // ---- upvalues (instack + idx, no name) ----
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative upval count".to_string());
    }
    let mut upvals: Vec<UpvalDesc> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let in_stack = r.u8()? != 0;
        let index = r.u8()?;
        upvals.push(UpvalDesc {
            in_stack,
            index,
            name: Box::from(""),
            read_only: false,
        });
    }

    // ---- nested protos ----
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative proto count".to_string());
    }
    let mut protos = Vec::with_capacity(n as usize);
    for _ in 0..n {
        protos.push(r_proto(r, heap, Some(source))?);
    }

    // ---- debug: lineinfo (per-PC i32 array), then locvars, then upval names ----
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative lineinfo count".to_string());
    }
    let mut puc_lines: Vec<u32> = Vec::with_capacity(n as usize);
    for _ in 0..n {
        // PUC stores lineinfo as int (4 bytes) per instruction; luna
        // stores u32. Negative line numbers shouldn't occur in valid
        // chunks (the parser only assigns 1..=LINEMAX), but if we see
        // one we clamp to 0 — "unknown line" — rather than fail.
        let ln = r_int(r)?;
        puc_lines.push(if ln < 0 { 0 } else { ln as u32 });
    }
    // Remap PUC's per-PUC-pc line numbers to luna's per-luna-pc layout via
    // the `luna_to_puc_pc` table. For 1:1 ops the remap is the identity; for
    // multi-inst lowerings (Step 4+) it duplicates the source line across each
    // emitted inst so debug.getinfo / error traces stay accurate.
    let lines: Vec<u32> = translated
        .luna_to_puc_pc
        .iter()
        .map(|&puc_pc| {
            puc_lines
                .get(puc_pc)
                .copied()
                // Multi-inst lowerings that emit beyond the PUC stream length
                // (shouldn't happen for 5.3 today) fall back to "unknown line".
                .unwrap_or(0)
        })
        .collect();
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative locvar count".to_string());
    }
    let mut locvars = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let name = match r_string(r)? {
            Some(b) => String::from_utf8_lossy(b).into_owned().into(),
            None => Box::from(""),
        };
        let start_pc = r_int(r)? as u32;
        let end_pc = r_int(r)? as u32;
        // PUC LocVar has no `reg` field — locvars are scope records,
        // not register assignments. luna's LocVar carries `reg` for
        // its own diag emission; PUC chunks don't have it, so we set
        // it to MAX (== "unknown") to avoid lying about the binding.
        locvars.push(LocVar {
            name,
            reg: u32::MAX,
            start_pc,
            end_pc,
        });
    }
    let n = r_int(r)?;
    if n < 0 {
        return Err("PUC 5.3 translator: negative upval-name count".to_string());
    }
    let n = n as usize;
    if n > upvals.len() {
        return Err("PUC 5.3 translator: more upval names than upvals".to_string());
    }
    for i in 0..n {
        let name = match r_string(r)? {
            Some(b) => String::from_utf8_lossy(b).into_owned().into(),
            None => Box::from(""),
        };
        upvals[i].name = name;
    }

    let env_upval_idx = upvals
        .iter()
        .take(u8::MAX as usize)
        .position(|u| &*u.name == "_ENV")
        .map_or(u8::MAX, |i| i as u8);
    // PUC's `max_stack` accounts only for its own register usage; multi-inst
    // lowerings (RK-on-B etc.) reserve one or more extra slots above it.
    let max_stack = max_stack_puc.saturating_add(translated.max_temp_bump);
    Ok(heap.adopt_proto(Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: translated.code.into_boxed_slice(),
        consts: consts.into_boxed_slice(),
        protos: protos.into_boxed_slice(),
        upvals: upvals.into_boxed_slice(),
        num_params,
        is_vararg,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack,
        lines: lines.into_boxed_slice(),
        source,
        line_defined,
        last_line_defined,
        locvars: locvars.into_boxed_slice(),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        traces: std::cell::RefCell::new(Vec::new()),
    }))
}

/// Entry point — wired into `super::puc::undump_puc` via the version-byte
/// dispatch. Caller has already validated the `\x1bLua` signature; we
/// re-check (cheap) so the per-dialect error surface is consistent if
/// someone calls us directly in tests.
pub(super) fn undump_puc_53(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    let mut r = Reader::at(bytes, 0);
    check_header(&mut r)?;
    // PUC writes `nupvalues` for the main closure before the recursive
    // LoadFunction; luna's `Proto` carries that on the Proto itself, so
    // we read+discard here and let the per-Proto upvalue list be the
    // source of truth.
    let _main_nupvals = r.u8()?;
    let proto = r_proto(&mut r, heap, None)?;
    if r.pos() != bytes.len() {
        return Err(format!(
            "PUC 5.3 translator: {} trailing bytes after main proto",
            bytes.len() - r.pos()
        ));
    }
    Ok(proto)
}

#[cfg(test)]
#[allow(clippy::identity_op, clippy::erasing_op)]
mod tests {
    use super::*;

    #[test]
    fn decode_53_layout_roundtrip() {
        // Build a PUC 5.3 instruction word by hand and verify the
        // decoder pulls the fields out at the right positions.
        // OP_MOVE (0), A=3, B=5, C=7  →  word = 0 | (3<<6) | (7<<14) | (5<<23)
        let word = 0u32 | (3 << 6) | (7 << 14) | (5 << 23);
        let i = decode_53(word);
        assert_eq!(i.op, 0);
        assert_eq!(i.a, 3);
        assert_eq!(i.b, 5);
        assert_eq!(i.c, 7);
    }

    #[test]
    fn decode_53_sbx_sign() {
        // sBx of 0 is encoded as MAXARG_sBx in the bx slot. Build
        // FORLOOP (39) A=0 sBx=-3 → bx = 131071 - 3 = 131068.
        let bx: u32 = (PUC53_MAX_ARG_SBX - 3) as u32;
        let word = 39u32
            | 0
            | ((bx & ((1 << PUC53_SIZE_C) - 1)) << PUC53_POS_C)
            | (((bx >> PUC53_SIZE_C) & ((1 << PUC53_SIZE_B) - 1)) << PUC53_POS_B);
        let i = decode_53(word);
        assert_eq!(i.op, 39);
        assert_eq!(i.sbx(), -3);
    }

    #[test]
    fn translate_move() {
        // OP_MOVE A=3 B=5  →  luna Op::Move A=3 B=5
        let word = 0u32 | (3 << 6) | (0 << 14) | (5 << 23);
        let inst = translate_inst(word).unwrap();
        assert_eq!(inst.op(), Op::Move);
        assert_eq!(inst.a(), 3);
        assert_eq!(inst.b(), 5);
    }

    #[test]
    fn translate_loadbool_false_skip() {
        // OP_LOADBOOL A=2 B=0 C=1  →  luna Op::LFalseSkip A=2
        let word = 3u32 | (2 << 6) | (1 << 14) | (0 << 23);
        let inst = translate_inst(word).unwrap();
        assert_eq!(inst.op(), Op::LFalseSkip);
        assert_eq!(inst.a(), 2);
    }

    #[test]
    fn translate_loadbool_true_skip_rejected() {
        // OP_LOADBOOL A=2 B=1 C=1  →  Err (luna has no LTrueSkip)
        let word = 3u32 | (2 << 6) | (1 << 14) | (1 << 23);
        assert!(translate_inst(word).unwrap_err().contains("LOADBOOL"));
    }

    #[test]
    fn translate_add_rk_c() {
        // OP_ADD A=4 B=5 C=K(7)  →  luna Op::Add A=4 B=5 C=7 k=1
        let c_field = PUC53_BITRK | 7;
        let word = 13u32 | (4 << 6) | (c_field << 14) | (5 << 23);
        let inst = translate_inst(word).unwrap();
        assert_eq!(inst.op(), Op::Add);
        assert_eq!(inst.a(), 4);
        assert_eq!(inst.b(), 5);
        assert_eq!(inst.c(), 7);
        assert!(inst.k());
    }

    #[test]
    fn translate_add_rk_b_rejected() {
        // OP_ADD with RK on B → baseline rejects
        let b_field = PUC53_BITRK | 5;
        let word = 13u32 | (4 << 6) | (7 << 14) | (b_field << 23);
        assert!(translate_inst(word).unwrap_err().contains("RK on B"));
    }

    #[test]
    fn translate_jmp_close_rejected() {
        // OP_JMP A=3 sBx=10 (close upvalues) → baseline rejects
        let bx: u32 = (PUC53_MAX_ARG_SBX + 10) as u32;
        let word = 30u32 | (3 << 6) | ((bx & 0x1FF) << 14) | (((bx >> 9) & 0x1FF) << 23);
        assert!(translate_inst(word).unwrap_err().contains("close-upvalues"));
    }

    #[test]
    fn translate_tforcall() {
        // OP_TFORCALL A=5 C=2 → luna Op::TForCall A=5 B=0 C=2
        let word = 41u32 | (5 << 6) | (2 << 14) | (0 << 23);
        let inst = translate_inst(word).unwrap();
        assert_eq!(inst.op(), Op::TForCall);
        assert_eq!(inst.a(), 5);
        assert_eq!(inst.b(), 0);
        assert_eq!(inst.c(), 2);
    }

    #[test]
    fn translate_tforloop_back_jump() {
        // OP_TFORLOOP A=7 sBx=-3 (back-jump 3) → luna Op::TForLoop A=5 Bx=3
        // PUC TFORLOOP.A=7 means iter_base=5 (R(A+1)=R(8)=first result;
        // R(A)=R(7)=ctrl slot). luna A = iter_base = 5; Bx = -(-3) = 3.
        let bx: u32 = (PUC53_MAX_ARG_SBX - 3) as u32;
        let word = 42u32 | (7 << 6) | ((bx & 0x1FF) << 14) | (((bx >> 9) & 0x1FF) << 23);
        let inst = translate_inst(word).unwrap();
        assert_eq!(inst.op(), Op::TForLoop);
        assert_eq!(inst.a(), 5);
        assert_eq!(inst.bx(), 3);
    }

    #[test]
    fn translate_tforloop_forward_jump_rejected() {
        // OP_TFORLOOP with sBx > 0 → rejected (PUC always emits backward)
        let bx: u32 = (PUC53_MAX_ARG_SBX + 3) as u32;
        let word = 42u32 | (7 << 6) | ((bx & 0x1FF) << 14) | (((bx >> 9) & 0x1FF) << 23);
        assert!(
            translate_inst(word)
                .unwrap_err()
                .contains("expected backward jump")
        );
    }

    #[test]
    fn translate_tforloop_low_a_rejected() {
        // OP_TFORLOOP A=1 (< 2, cannot convert iter_base via A-2) → rejected
        let bx: u32 = (PUC53_MAX_ARG_SBX - 1) as u32;
        let word = 42u32 | (1 << 6) | ((bx & 0x1FF) << 14) | (((bx >> 9) & 0x1FF) << 23);
        assert!(translate_inst(word).unwrap_err().contains("< 2"));
    }

    // ---------------------------------------------------------------------
    // Phase 4 PU Wave 2 — PC remap infra + RK-on-B routing
    // ---------------------------------------------------------------------
    //
    // The legacy `translate_inst` tests above still cover the 1:1 fast path
    // (and the rejection it emits for RK-on-B / close-upvalues JMP). The
    // tests below exercise the full `translate_code` path that the embedder
    // hits via `r_proto`, including the multi-inst lowerings that lift the
    // earlier `translate_inst` rejections.

    /// Build a PUC 5.3 iABC instruction word.
    fn enc_iabc(op: u8, a: u32, b: u32, c: u32) -> u32 {
        (op as u32) | ((a & 0xFF) << 6) | ((c & 0x1FF) << 14) | ((b & 0x1FF) << 23)
    }

    /// Build a PUC 5.3 iAsBx instruction word.
    fn enc_iasbx(op: u8, a: u32, sbx: i32) -> u32 {
        let bx = (sbx + PUC53_MAX_ARG_SBX) as u32;
        (op as u32) | ((a & 0xFF) << 6) | ((bx & 0x1FF) << 14) | (((bx >> 9) & 0x1FF) << 23)
    }

    #[test]
    fn pc_remap_identity_on_one_to_one_stream() {
        // Pure 1:1 stream — `translate_code` should give back exactly the
        // same instructions as `translate_inst`, with identity pc maps.
        let words = vec![
            enc_iabc(OP_MOVE, 0, 1, 0),    // MOVE 0 1
            enc_iabc(OP_LOADNIL, 0, 0, 0), // LOADNIL 0 0
            enc_iabc(OP_RETURN, 0, 1, 0),  // RETURN 0 1
        ];
        let t = translate_code(&words).unwrap();
        assert_eq!(t.code.len(), 3);
        assert_eq!(t.puc_to_luna_pc, vec![Some(0), Some(1), Some(2)]);
        assert_eq!(t.luna_to_puc_pc, vec![0, 1, 2]);
        assert_eq!(t.max_temp_bump, 0);
    }

    #[test]
    fn jump_remap_through_fixup_loop() {
        // JMP +1 at pc 0 over a MOVE → resolves to luna sJ = +1 on the
        // unshifted stream (identity remap).
        let words = vec![
            enc_iasbx(OP_JMP, 0, 1),    // JMP +1 → target = pc 2
            enc_iabc(OP_MOVE, 0, 1, 0), // skipped
            enc_iabc(OP_RETURN, 0, 1, 0),
        ];
        let t = translate_code(&words).unwrap();
        assert_eq!(t.code.len(), 3);
        assert_eq!(t.code[0].op(), Op::Jmp);
        assert_eq!(t.code[0].sj(), 1);
    }

    /// Shape-validate the RK-on-B lowering for a single arith opcode.
    /// Builds `OP A=2 B=K(5) C=3`, runs `translate_code`, and checks that
    /// the emitted pair is `LoadK tmp 5; OP 2 tmp 3` with the right tmp
    /// register (`max(A, C) + 1 = 4`) and `max_temp_bump = 5`.
    fn assert_rk_on_b_lowered(puc_op: u8, luna_op: Op) {
        let b_field = PUC53_BITRK | 5; // K-pool index 5
        let words = vec![enc_iabc(puc_op, 2, b_field, 3)];
        let t = translate_code(&words).expect("RK-on-B arith must lower cleanly");
        assert_eq!(t.code.len(), 2, "{puc_op}: expected 2-inst lowering");
        // First inst: LoadK tmp 5, where tmp = max(A=2, C=3) + 1 = 4.
        assert_eq!(t.code[0].op(), Op::LoadK);
        assert_eq!(t.code[0].a(), 4);
        assert_eq!(t.code[0].bx(), 5);
        // Second inst: <op> A=2 B=tmp=4 C=3, k bit clear (C is a register).
        assert_eq!(t.code[1].op(), luna_op);
        assert_eq!(t.code[1].a(), 2);
        assert_eq!(t.code[1].b(), 4);
        assert_eq!(t.code[1].c(), 3);
        assert!(!t.code[1].k());
        // pc maps: single PUC pc → first luna pc; both luna pcs cite puc_pc=0.
        assert_eq!(t.puc_to_luna_pc, vec![Some(0)]);
        assert_eq!(t.luna_to_puc_pc, vec![0, 0]);
        // max_temp_bump = tmp + 1 = 5 (one extra slot reserved past PUC frame).
        assert_eq!(t.max_temp_bump, 5);
    }

    #[test]
    fn rk_on_b_lowering_add() {
        assert_rk_on_b_lowered(OP_ADD, Op::Add);
    }
    #[test]
    fn rk_on_b_lowering_sub() {
        assert_rk_on_b_lowered(OP_SUB, Op::Sub);
    }
    #[test]
    fn rk_on_b_lowering_mul() {
        assert_rk_on_b_lowered(OP_MUL, Op::Mul);
    }
    #[test]
    fn rk_on_b_lowering_mod() {
        assert_rk_on_b_lowered(OP_MOD, Op::Mod);
    }
    #[test]
    fn rk_on_b_lowering_pow() {
        assert_rk_on_b_lowered(OP_POW, Op::Pow);
    }
    #[test]
    fn rk_on_b_lowering_div() {
        assert_rk_on_b_lowered(OP_DIV, Op::Div);
    }
    #[test]
    fn rk_on_b_lowering_idiv() {
        assert_rk_on_b_lowered(OP_IDIV, Op::IDiv);
    }
    #[test]
    fn rk_on_b_lowering_band() {
        assert_rk_on_b_lowered(OP_BAND, Op::BAnd);
    }
    #[test]
    fn rk_on_b_lowering_bor() {
        assert_rk_on_b_lowered(OP_BOR, Op::BOr);
    }
    #[test]
    fn rk_on_b_lowering_bxor() {
        assert_rk_on_b_lowered(OP_BXOR, Op::BXor);
    }
    #[test]
    fn rk_on_b_lowering_shl() {
        assert_rk_on_b_lowered(OP_SHL, Op::Shl);
    }
    #[test]
    fn rk_on_b_lowering_shr() {
        assert_rk_on_b_lowered(OP_SHR, Op::Shr);
    }

    #[test]
    fn rk_on_b_propagates_c_k_flag() {
        // OP_ADD A=2 B=K(5) C=K(7): both sides constants. lowering should set
        // the k bit on the emitted Add (since C is still RK) while the LoadK
        // synthesizes B.
        let b_field = PUC53_BITRK | 5;
        let c_field = PUC53_BITRK | 7;
        let words = vec![enc_iabc(OP_ADD, 2, b_field, c_field)];
        let t = translate_code(&words).unwrap();
        assert_eq!(t.code.len(), 2);
        assert_eq!(t.code[0].op(), Op::LoadK);
        assert_eq!(t.code[0].bx(), 5);
        assert_eq!(t.code[1].op(), Op::Add);
        assert!(t.code[1].k(), "C side stays RK, k bit must propagate");
        assert_eq!(t.code[1].c(), 7);
    }

    #[test]
    fn rk_on_b_shifts_downstream_jump_target() {
        // Stream:
        //   pc 0: ADD A=0 B=K(0) C=1   (lowers to 2 luna insts → shifts)
        //   pc 1: JMP +1               (target PUC pc 3 = RETURN)
        //   pc 2: MOVE 0 1             (skipped by JMP)
        //   pc 3: RETURN 0 1
        //
        // After lowering, luna stream is:
        //   luna 0: LoadK tmp=2 0
        //   luna 1: Add 0 tmp 1
        //   luna 2: Jmp <fixup>       (was puc 1; target was puc 3 = luna 4)
        //   luna 3: Move 0 1
        //   luna 4: Return 0 1
        // The fixup loop should set sJ on luna 2 = (4 - (2+1)) = 1.
        let b_field = PUC53_BITRK | 0;
        let words = vec![
            enc_iabc(OP_ADD, 0, b_field, 1),
            enc_iasbx(OP_JMP, 0, 1),
            enc_iabc(OP_MOVE, 0, 1, 0),
            enc_iabc(OP_RETURN, 0, 1, 0),
        ];
        let t = translate_code(&words).unwrap();
        assert_eq!(t.code.len(), 5);
        assert_eq!(t.code[0].op(), Op::LoadK);
        assert_eq!(t.code[1].op(), Op::Add);
        assert_eq!(t.code[2].op(), Op::Jmp);
        assert_eq!(
            t.code[2].sj(),
            1,
            "JMP target must remap to luna pc 4 after lowering shift"
        );
        assert_eq!(t.code[3].op(), Op::Move);
        assert_eq!(t.code[4].op(), Op::Return);
        // puc_to_luna_pc: pc 0 = luna 0 (LoadK first); pc 1 = luna 2 (Jmp);
        // pc 2 = luna 3; pc 3 = luna 4.
        assert_eq!(t.puc_to_luna_pc, vec![Some(0), Some(2), Some(3), Some(4)]);
    }
}
