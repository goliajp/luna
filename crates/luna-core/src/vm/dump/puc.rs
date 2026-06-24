//! Per-dialect PUC bytecode → luna `Proto` translators.
//!
//! Phase LB Wave 1 (this file as a stub): owns the magic-byte → dialect
//! dispatch table. Each per-dialect entry currently returns
//! `Err("PUC Lua N.M bytecode loading not yet implemented (Phase LBN)")`;
//! Wave 2 will land the five real translators in parallel:
//!
//! - `puc_51.rs` — PUC 5.1 (6-bit opcode; `_ENV` synth for `GETGLOBAL`)
//! - `puc_52.rs` — PUC 5.2 (6-bit opcode; native `_ENV`)
//! - `puc_53.rs` — PUC 5.3 (6-bit opcode; Int subtype + bitwise + `IDIV`)
//! - `puc_54.rs` — PUC 5.4 (7-bit opcode matches luna; K/I/MMBIN lowering)
//! - `puc_55.rs` — PUC 5.5 (7-bit opcode; `OP_GLOBAL`/`DEFGLOBAL` lowering)
//!
//! See `.dev/rfcs/v1.3-audit-puc-luac-formats.md` for the full plan.

#![allow(dead_code)] // Wave 2 wires these into `super::undump`; stubs only for now.

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};

/// Magic-byte dispatcher: peek `bytes[4]` (the PUC version byte) and route
/// to the matching per-dialect undumper. Caller has already confirmed the
/// `\x1bLua` signature at bytes 0..4.
pub(super) fn undump_puc(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    if bytes.len() < 5 {
        return Err("truncated PUC binary chunk".to_string());
    }
    match bytes[4] {
        0x51 => undump_puc_51(bytes, heap),
        0x52 => undump_puc_52(bytes, heap),
        0x53 => undump_puc_53(bytes, heap),
        0x54 => undump_puc_54(bytes, heap),
        0x55 => undump_puc_55(bytes, heap),
        v => Err(format!(
            "unsupported PUC Lua version byte 0x{v:02x} (expected 0x51..0x55)"
        )),
    }
}

fn undump_puc_51(_bytes: &[u8], _heap: &mut Heap) -> Result<Gc<Proto>, String> {
    Err("PUC Lua 5.1 bytecode loading not yet implemented (Phase LB6)".to_string())
}

fn undump_puc_52(_bytes: &[u8], _heap: &mut Heap) -> Result<Gc<Proto>, String> {
    Err("PUC Lua 5.2 bytecode loading not yet implemented (Phase LB5)".to_string())
}

fn undump_puc_53(_bytes: &[u8], _heap: &mut Heap) -> Result<Gc<Proto>, String> {
    Err("PUC Lua 5.3 bytecode loading not yet implemented (Phase LB4)".to_string())
}

fn undump_puc_54(_bytes: &[u8], _heap: &mut Heap) -> Result<Gc<Proto>, String> {
    Err("PUC Lua 5.4 bytecode loading not yet implemented (Phase LB3)".to_string())
}

fn undump_puc_55(_bytes: &[u8], _heap: &mut Heap) -> Result<Gc<Proto>, String> {
    Err("PUC Lua 5.5 bytecode loading not yet implemented (Phase LB7)".to_string())
}
