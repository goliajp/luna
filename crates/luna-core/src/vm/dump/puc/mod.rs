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

#![allow(dead_code)] // remaining dialect stubs may not be wired yet

mod puc_51;
mod puc_52;
mod puc_53;
mod puc_54;
mod puc_55;

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
