//! Binary-chunk header check, shared by luna's own format and the PUC
//! translators.
//!
//! A header is compared byte by byte against the expected one, in the order
//! PUC 5.3+ `checkHeader` reads it: the first differing byte decides the
//! [`Bad`] category (from the field it falls in), and a chunk that ends
//! before a difference is `Truncated`. Each dialect's layout lists its
//! fields as `(end, category)`, a field covering the bytes from the previous
//! end up to `end`.

use super::error::{Bad, Num};

type Layout = [(usize, Bad)];

/// PUC 5.1 `luaU_header`: signature, version, format, endianness, the sizes
/// of `int`, `size_t`, `Instruction`, `lua_Number`, and the integral flag.
pub(super) const LAYOUT_51: &Layout = &[
    (4, Bad::NotBinary),
    (5, Bad::Version),
    (6, Bad::Format),
    (7, Bad::NumFormat(Num::Integer)),
    (8, Bad::Size(Num::Int)),
    (9, Bad::Size(Num::SizeT)),
    (10, Bad::Size(Num::Instruction)),
    (11, Bad::Size(Num::Number)),
    (12, Bad::NumFormat(Num::Number)),
];

/// PUC 5.2: 5.1's twelve bytes plus the `LUAC_TAIL` guard.
pub(super) const LAYOUT_52: &Layout = &[
    (4, Bad::NotBinary),
    (5, Bad::Version),
    (6, Bad::Format),
    (7, Bad::NumFormat(Num::Integer)),
    (8, Bad::Size(Num::Int)),
    (9, Bad::Size(Num::SizeT)),
    (10, Bad::Size(Num::Instruction)),
    (11, Bad::Size(Num::Number)),
    (12, Bad::NumFormat(Num::Number)),
    (18, Bad::Corrupted),
];

/// PUC 5.3: signature, version, format, `LUAC_DATA`, five sizes,
/// `LUAC_INT`, `LUAC_NUM`.
pub(super) const LAYOUT_53: &Layout = &[
    (4, Bad::NotBinary),
    (5, Bad::Version),
    (6, Bad::Format),
    (12, Bad::Corrupted),
    (13, Bad::Size(Num::Int)),
    (14, Bad::Size(Num::SizeT)),
    (15, Bad::Size(Num::Instruction)),
    (16, Bad::Size(Num::Integer)),
    (17, Bad::Size(Num::Number)),
    (25, Bad::NumFormat(Num::Integer)),
    (33, Bad::NumFormat(Num::Number)),
];

/// PUC 5.4: 5.3's layout without the `int` and `size_t` sizes.
pub(super) const LAYOUT_54: &Layout = &[
    (4, Bad::NotBinary),
    (5, Bad::Version),
    (6, Bad::Format),
    (12, Bad::Corrupted),
    (13, Bad::Size(Num::Instruction)),
    (14, Bad::Size(Num::Integer)),
    (15, Bad::Size(Num::Number)),
    (23, Bad::NumFormat(Num::Integer)),
    (31, Bad::NumFormat(Num::Number)),
];

/// PUC 5.5 `checknum`: each type's size byte followed by its check value.
pub(super) const LAYOUT_55: &Layout = &[
    (4, Bad::NotBinary),
    (5, Bad::Version),
    (6, Bad::Format),
    (12, Bad::Corrupted),
    (13, Bad::Size(Num::Int)),
    (17, Bad::NumFormat(Num::Int)),
    (18, Bad::Size(Num::Instruction)),
    (22, Bad::NumFormat(Num::Instruction)),
    (23, Bad::Size(Num::Integer)),
    (31, Bad::NumFormat(Num::Integer)),
    (32, Bad::Size(Num::Number)),
    (40, Bad::NumFormat(Num::Number)),
];

/// Compare `bytes`' header with `expected`, laid out as `layout`.
pub(super) fn check(bytes: &[u8], expected: &[u8], layout: &Layout) -> Result<(), Bad> {
    debug_assert_eq!(layout.last().map(|f| f.0), Some(expected.len()));
    for (i, &want) in expected.iter().enumerate() {
        let Some(&got) = bytes.get(i) else {
            return Err(Bad::Truncated);
        };
        if got != want {
            let field = layout
                .iter()
                .find(|f| i < f.0)
                .expect("the layout covers the header");
            return Err(field.1.clone());
        }
    }
    Ok(())
}
