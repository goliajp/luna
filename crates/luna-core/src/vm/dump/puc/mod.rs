//! PUC bytecode → luna `Proto` translators, one module per dialect.
//!
//! Each dialect module reads its own chunk format into a
//! [`lower::RawProto`] tree; the code is then lowered into luna's ISA by
//! [`classic`] (5.2, 5.3), [`modern`] (5.4, 5.5) or `puc_51` itself, all on
//! the shared machinery in [`lower`]. `lower`'s module docs list what luna's
//! interpreter trusts a translated proto to satisfy.

mod classic;
mod lower;
mod modern;
mod puc_51;
mod puc_52;
mod puc_53;
mod puc_54;
mod puc_55;

use crate::runtime::function::Proto;
use crate::runtime::heap::{Gc, Heap};

/// Route a `\x1bLua` chunk to its dialect's undumper by the version byte.
pub(super) fn undump_puc(bytes: &[u8], heap: &mut Heap) -> Result<Gc<Proto>, String> {
    match bytes.get(4) {
        Some(0x51) => puc_51::undump(bytes, heap),
        Some(0x52) => puc_52::undump(bytes, heap),
        Some(0x53) => puc_53::undump_puc_53(bytes, heap),
        Some(0x54) => puc_54::undump(bytes, heap),
        Some(0x55) => puc_55::undump_puc_55(bytes, heap),
        Some(v) => Err(format!(
            "unsupported PUC Lua version byte 0x{v:02x} (expected 0x51..0x55)"
        )),
        None => Err("truncated PUC binary chunk".to_string()),
    }
}
