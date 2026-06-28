//! Per-dialect PUC bytecode → luna `Proto` translators.
//!
//! Phase LB Wave 1 owned the magic-byte → dialect dispatch table.
//! Wave 2 fills in the five real translators in parallel modules:
//!
//! - `puc_51.rs` — PUC 5.1 (6-bit opcode; `_ENV` synth for `GETGLOBAL`)
//! - `puc_52.rs` — PUC 5.2 (6-bit opcode; native `_ENV`)
//! - `puc_53.rs` — PUC 5.3 (6-bit opcode; Int subtype + bitwise + `IDIV`)
//! - `puc_54.rs` — PUC 5.4 (7-bit opcode matches luna; K/I/MMBIN lowering;
//!   RLE lineinfo)
//! - `puc_55.rs` — PUC 5.5 (7-bit opcode; PUC MSB-first varint header;
//!   lowers MMBIN / VARARGPREP / K-imm / I-imm into luna's 65-op set)
//!
//! See `.dev/rfcs/v1.3-audit-puc-luac-formats.md` for the full plan.
//!
//! ## Shared lowering helpers (Phase 4 PU Wave 1)
//!
//! The five dialect modules share three classes of opcode-lowering work that
//! every translator hits: materializing a constant from the K pool into a
//! temporary register so a luna arith op can consume it (`lower_k_via_tmp`),
//! materializing a signed-8-bit immediate as a `LoadI` into a temporary so a
//! luna arith op can consume it (`lower_i_imm`), and scanning a PUC code
//! stream for the JMP→TFORCALL→TFORLOOP triad so the synthesized luna
//! `TForPrep` can be injected at the right pc (`scan_tforprep_sites`). The
//! helpers live here to avoid four-way bug-fix forks across the dialect
//! modules (audit risk R2). Wave 1 refactors `puc_54.rs` to call `lower_i_imm`
//! with zero behavior change; Wave 2 will wire the remaining dialects.

#![allow(dead_code)] // remaining dialect stubs may not be wired yet

mod puc_51;
mod puc_52;
mod puc_53;
mod puc_54;
mod puc_55;

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};
use crate::vm::isa::{Inst, Op};

/// Magic-byte dispatcher: peek `bytes[4]` (the PUC version byte) and route
/// to the matching per-dialect undumper. Caller has already confirmed the
/// `\x1bLua` signature at bytes 0..4.
pub(super) fn undump_puc(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    if bytes.len() < 5 {
        return Err("truncated PUC binary chunk".to_string());
    }
    match bytes[4] {
        0x51 => puc_51::undump(bytes, heap),
        0x52 => puc_52::undump(bytes, heap),
        0x53 => puc_53::undump_puc_53(bytes, heap),
        0x54 => puc_54::undump(bytes, heap),
        0x55 => puc_55::undump_puc_55(bytes, heap),
        v => Err(format!(
            "unsupported PUC Lua version byte 0x{v:02x} (expected 0x51..0x55)"
        )),
    }
}

// ---------------------------------------------------------------------------
// Shared lowering helpers (Phase 4 PU Wave 1).
//
// These three helpers exist so the five dialect modules do not each grow
// their own copy of the same lowering shape. Each helper is pure (no I/O,
// no GC interaction); they only manipulate luna `Inst` values and a
// `max_temp_bump` counter. The dialect modules own register allocation
// policy (which register to claim as `tmp`) and pass the chosen `tmp`
// through; the helpers handle the bounds check and the emit.
// ---------------------------------------------------------------------------

/// Lower a PUC arith op whose constant operand is on the **B side**
/// (`R[A] := K[k_idx] <op> R[C]/K[C]`) to a luna `LoadK tmp k_idx; OP a tmp c`
/// pair. luna's arith ops only accept a `k` flag on the C operand, so when
/// the PUC dialect places the constant on B we must materialize it via a
/// temp register first.
///
/// `tmp` is chosen by the caller (typically `max(a, c) + 1`) and the helper
/// bumps `*max_temp_bump` to keep at least one slot live past the frame's
/// declared `max_stack`. Returns `Err` if `tmp` would exceed the 8-bit
/// register field (255) or if `k_idx` would exceed the 17-bit `Bx` field.
///
/// Used by PUC 5.1 / 5.2 / 5.3 (which have the `RK(B)` encoding). PUC 5.4
/// and 5.5 always place the constant on C and use `OP_ADDK`-style direct
/// k-bit translation, so this helper is unused there.
pub(super) fn lower_k_via_tmp(
    op: Op,
    a: u32,
    k_idx: u32,
    c: u32,
    c_is_k: bool,
    tmp: u32,
    max_temp_bump: &mut u8,
) -> Result<[Inst; 2], String> {
    if tmp > 0xFF {
        return Err(format!(
            "lower_k_via_tmp: temp register {tmp} exceeds 255 (op={op:?}, a={a}, k_idx={k_idx})"
        ));
    }
    if k_idx > 0x1FFFF {
        return Err(format!(
            "lower_k_via_tmp: K-pool index {k_idx} exceeds 17-bit Bx field"
        ));
    }
    *max_temp_bump = (*max_temp_bump).max(tmp as u8 + 1);
    Ok([
        Inst::iabx(Op::LoadK, tmp, k_idx),
        Inst::iabc(op, a, tmp, c, c_is_k),
    ])
}

/// Lower a PUC I-imm arith op (`R[A] := R[B] <op> sC` where `sC` is a signed
/// 8-bit literal) to a luna `LoadI tmp sC; OP a b tmp` pair. luna has no
/// I-immediate arith form, so every I-imm op must materialize the literal
/// into a register first.
///
/// `tmp` is chosen by the caller (puc_54's policy is `max(a, b) + 1` to avoid
/// clobbering either source) and the helper bumps `*max_temp_bump` to keep
/// the slot reserved. Returns `Err` if `tmp` would exceed 255.
///
/// Covers the **3-operand same-order shape** (PUC ADDI / SHRI in 5.4 and
/// 5.5). Sites with operand-swap shapes (PUC SHLI: `R[A] := sC << R[B]`) or
/// flag-encoded shapes (PUC EQI / LTI / LEI / GTI / GEI: skip-on-condition)
/// stay inline in the dialect module — their second instruction is not a
/// drop-in for this helper's emit.
pub(super) fn lower_i_imm(
    op: Op,
    a: u32,
    b: u32,
    sc: i32,
    tmp: u32,
    max_temp_bump: &mut u8,
) -> Result<[Inst; 2], String> {
    if tmp > 0xFF {
        return Err(format!(
            "lower_i_imm: temp register {tmp} exceeds 255 (op={op:?}, a={a}, b={b}, sc={sc})"
        ));
    }
    *max_temp_bump = (*max_temp_bump).max(tmp as u8 + 1);
    Ok([
        Inst::iasbx(Op::LoadI, tmp, sc),
        Inst::iabc(op, a, b, tmp, false),
    ])
}

/// Scan a PUC code stream for the sites where luna's `Op::TForPrep` must be
/// injected. PUC 5.1 / 5.2 / 5.5 emit a JMP **into** the TFORCALL/TFORLOOP
/// body (the jump shape `JMP <loop_test>; <body>; TFORCALL; TFORLOOP` —
/// the first JMP is the prep-skip-to-test entry). luna's calling convention
/// puts a `TForPrep` at that JMP site instead of the JMP, so the dialect
/// module rewrites the JMP slot to `TForPrep iter_base offset_to_tforloop`.
///
/// Returns a map of `puc_pc_of_jmp → iter_base_register_A`. The caller
/// looks up each JMP it would otherwise translate; if the JMP's pc is in the
/// returned map, it emits `TForPrep` with the corresponding iter_base
/// instead of a plain `Jmp`.
///
/// The opcode numbers (`tforcall_op`, `tforloop_op`, `jmp_op`) differ between
/// 5.1 / 5.2 / 5.3 / 5.4 / 5.5 — the caller passes its own dialect's values.
/// `decode_op`, `decode_a` and `decode_sbx` decode the bit-packed fields
/// from a raw `u32` instruction word; each dialect supplies its own decoders
/// because 5.1/5.2/5.3 use 6-bit opcodes while 5.4/5.5 use 7-bit.
pub(super) fn scan_tforprep_sites(
    words: &[u32],
    tforcall_op: u8,
    tforloop_op: u8,
    jmp_op: u8,
    decode_op: impl Fn(u32) -> u8,
    decode_a: impl Fn(u32) -> u32,
    decode_sbx: impl Fn(u32) -> i32,
) -> std::collections::HashMap<usize, u32> {
    let mut out = std::collections::HashMap::new();
    for (pc, &w) in words.iter().enumerate() {
        if decode_op(w) != jmp_op {
            continue;
        }
        // PUC JMP encodes a signed displacement *from next pc*. Compute the
        // target pc and look for a TFORCALL there (with a TFORLOOP one slot
        // after). If the JMP points at TFORCALL it's the loop-test entry
        // we synthesize `TForPrep` for.
        let next_pc = pc as i64 + 1;
        let target = next_pc + decode_sbx(w) as i64;
        if target < 0 {
            continue;
        }
        let target = target as usize;
        if target >= words.len() {
            continue;
        }
        if decode_op(words[target]) != tforcall_op {
            continue;
        }
        if target + 1 >= words.len() || decode_op(words[target + 1]) != tforloop_op {
            continue;
        }
        // TFORCALL.A is the iter_base register.
        out.insert(pc, decode_a(words[target]));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    // --- lower_k_via_tmp ---

    #[test]
    fn lower_k_via_tmp_emits_pair() {
        let mut bump = 0u8;
        let pair = lower_k_via_tmp(Op::Add, 5, 7, 3, false, 6, &mut bump).unwrap();
        assert_eq!(pair[0].op(), Op::LoadK);
        assert_eq!(pair[0].a(), 6);
        assert_eq!(pair[0].bx(), 7);
        assert_eq!(pair[1].op(), Op::Add);
        assert_eq!(pair[1].a(), 5);
        assert_eq!(pair[1].b(), 6);
        assert_eq!(pair[1].c(), 3);
        assert!(!pair[1].k());
        assert_eq!(bump, 7);
    }

    #[test]
    fn lower_k_via_tmp_propagates_c_k_flag() {
        // c_is_k=true means C is a K-pool index, set the k bit on the arith.
        let mut bump = 0u8;
        let pair = lower_k_via_tmp(Op::Sub, 0, 2, 4, true, 1, &mut bump).unwrap();
        assert_eq!(pair[1].op(), Op::Sub);
        assert!(pair[1].k(), "c_is_k=true must propagate to k bit");
    }

    #[test]
    fn lower_k_via_tmp_rejects_oversized_tmp() {
        let mut bump = 0u8;
        let err = lower_k_via_tmp(Op::Add, 0, 1, 2, false, 256, &mut bump).unwrap_err();
        assert!(err.contains("exceeds 255"), "got: {err}");
        assert_eq!(bump, 0, "bump must not change on error");
    }

    #[test]
    fn lower_k_via_tmp_keeps_max_bump_monotonic() {
        // A larger pre-existing bump must not be lowered by a smaller alloc.
        let mut bump = 10u8;
        lower_k_via_tmp(Op::Add, 0, 1, 2, false, 3, &mut bump).unwrap();
        assert_eq!(bump, 10, "bump must stay at running max");
    }

    // --- lower_i_imm ---

    #[test]
    fn lower_i_imm_emits_pair() {
        let mut bump = 0u8;
        let pair = lower_i_imm(Op::Add, 5, 3, 42, 6, &mut bump).unwrap();
        assert_eq!(pair[0].op(), Op::LoadI);
        assert_eq!(pair[0].a(), 6);
        assert_eq!(pair[0].sbx(), 42);
        assert_eq!(pair[1].op(), Op::Add);
        assert_eq!(pair[1].a(), 5);
        assert_eq!(pair[1].b(), 3);
        assert_eq!(pair[1].c(), 6);
        assert!(!pair[1].k(), "I-imm arith never sets the k bit");
        assert_eq!(bump, 7);
    }

    #[test]
    fn lower_i_imm_handles_negative_imm() {
        let mut bump = 0u8;
        let pair = lower_i_imm(Op::Shr, 0, 1, -5, 2, &mut bump).unwrap();
        assert_eq!(pair[0].sbx(), -5);
        assert_eq!(pair[1].op(), Op::Shr);
    }

    #[test]
    fn lower_i_imm_rejects_oversized_tmp() {
        let mut bump = 0u8;
        let err = lower_i_imm(Op::Add, 0, 1, 0, 256, &mut bump).unwrap_err();
        assert!(err.contains("exceeds 255"), "got: {err}");
        assert_eq!(bump, 0, "bump must not change on error");
    }

    #[test]
    fn lower_i_imm_keeps_max_bump_monotonic() {
        let mut bump = 8u8;
        lower_i_imm(Op::Add, 0, 1, 0, 3, &mut bump).unwrap();
        assert_eq!(bump, 8, "bump must stay at running max");
    }

    // --- scan_tforprep_sites ---

    // Synthetic 6-bit-op PUC-shape encoding for tests: op:6 | a:8 | sBx:18
    // biased. Only used by tests below; real dialects bring their own
    // decoders.
    const TEST_BIAS_SBX: i32 = (1 << 17) - 1;
    const TEST_JMP: u8 = 30;
    const TEST_TFORCALL: u8 = 41;
    const TEST_TFORLOOP: u8 = 42;

    fn enc_iabc(op: u8, a: u32) -> u32 {
        (op as u32 & 0x3F) | ((a & 0xFF) << 6)
    }
    fn enc_iasbx(op: u8, a: u32, sbx: i32) -> u32 {
        let bx = (sbx + TEST_BIAS_SBX) as u32;
        (op as u32 & 0x3F) | ((a & 0xFF) << 6) | ((bx & 0x3FFFF) << 14)
    }
    fn dec_op(w: u32) -> u8 {
        (w & 0x3F) as u8
    }
    fn dec_a(w: u32) -> u32 {
        (w >> 6) & 0xFF
    }
    fn dec_sbx(w: u32) -> i32 {
        ((w >> 14) & 0x3FFFF) as i32 - TEST_BIAS_SBX
    }

    #[test]
    fn scan_finds_jmp_then_tforcall_pair() {
        // Layout:
        //   pc 0: JMP +2     → next pc 1, target 3 = TFORCALL
        //   pc 1: <body>
        //   pc 2: <body>
        //   pc 3: TFORCALL.A=5
        //   pc 4: TFORLOOP.A=7
        let code = vec![
            enc_iasbx(TEST_JMP, 0, 2),
            enc_iabc(0, 0),
            enc_iabc(0, 0),
            enc_iabc(TEST_TFORCALL, 5),
            enc_iabc(TEST_TFORLOOP, 7),
        ];
        let sites = scan_tforprep_sites(
            &code,
            TEST_TFORCALL,
            TEST_TFORLOOP,
            TEST_JMP,
            dec_op,
            dec_a,
            dec_sbx,
        );
        assert_eq!(sites.len(), 1);
        assert_eq!(sites.get(&0), Some(&5));
    }

    #[test]
    fn scan_ignores_jmp_not_targeting_tforcall() {
        // Plain JMP at pc 0 targets pc 2 which is not TFORCALL — no entry.
        let code = vec![enc_iasbx(TEST_JMP, 0, 1), enc_iabc(0, 0), enc_iabc(0, 0)];
        let sites = scan_tforprep_sites(
            &code,
            TEST_TFORCALL,
            TEST_TFORLOOP,
            TEST_JMP,
            dec_op,
            dec_a,
            dec_sbx,
        );
        assert!(sites.is_empty());
    }
}
