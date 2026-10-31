//! Where compiled code finds the object header fields it reads: they move
//! with the header's layout and the collector's color bits, so they are
//! not part of the public API.

use crate::runtime::Table;
use crate::runtime::heap::GcHeader;
use crate::runtime::string::LuaStr;

/// Byte offset of a string's `u32` byte length.
pub const STR_LEN_OFFSET: usize =
    std::mem::offset_of!(LuaStr, hdr) + std::mem::offset_of!(GcHeader, aux);

/// Byte offset of the table header's flag byte: an inline store of a
/// collectable value tests [`TABLE_SLOW_MASK`] there (a black table takes
/// the write barrier, on the slow path).
pub const TABLE_FLAGS_OFFSET: usize = std::mem::offset_of!(Table, hdr) + GcHeader::FLAGS_OFFSET;

/// The bit of the flag byte set on a black or read-only table.
pub const TABLE_SLOW_MASK: u8 = crate::runtime::heap::SLOW;
