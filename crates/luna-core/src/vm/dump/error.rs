//! Why a binary chunk failed to load, and how each dialect says it.
//!
//! The loaders (luna's own format, the PUC translators, the verifier)
//! report a [`Bad`] in the categories PUC's `lundump.c` distinguishes. The
//! message is rendered in the *running* dialect's wording, since that is the
//! `lundump.c` a PUC build of that dialect would have run:
//!
//! | | frame | truncated |
//! |---|---|---|
//! | 5.1 | `<why> in precompiled chunk` | `unexpected end` |
//! | 5.2, 5.3 | `<why> precompiled chunk` | `truncated` |
//! | 5.4, 5.5 | `bad binary format (<why>)` | `truncated chunk` |
//!
//! `load` puts `lundump.c`'s chunk name in front (see [`lundump_name`]).
//! A chunk PUC would have loaded but luna refuses (the verifier, a shape
//! luna's instruction set cannot hold) has no PUC wording of its own; it is
//! reported in the dialect's nearest category (5.1 `bad code`, 5.2/5.3
//! `corrupted`, 5.4/5.5 the bare `bad binary format`) with luna's detail in
//! parentheses after it.

use crate::version::LuaVersion;

/// A number the header records the C type of, as `lundump.c` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Num {
    Int,
    SizeT,
    Instruction,
    Integer,
    Number,
}

/// The reason a chunk was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Bad {
    /// The chunk ends before a read completes.
    Truncated,
    /// The signature after the escape byte is not `Lua`.
    NotBinary,
    /// The version byte names another Lua.
    Version,
    /// The format byte is not the official one.
    Format,
    /// `LUAC_DATA` (or 5.2's tail) is damaged.
    Corrupted,
    /// The size the header gives for a C type differs.
    Size(Num),
    /// The header's check value of a type does not read back (endianness,
    /// integer or float representation).
    NumFormat(Num),
    /// A varint or count does not fit.
    IntOverflow,
    /// A constant's type tag names no constant type.
    Constant,
    /// A body the reader or the verifier refuses, with luna's detail.
    Code(String),
}

impl From<String> for Bad {
    fn from(detail: String) -> Bad {
        Bad::Code(detail)
    }
}

impl From<&str> for Bad {
    fn from(detail: &str) -> Bad {
        Bad::Code(detail.to_string())
    }
}

impl Bad {
    /// The message after `lundump.c`'s `<name>: `, in `version`'s wording.
    pub(super) fn render(&self, version: LuaVersion) -> String {
        let framed = match version {
            LuaVersion::Lua51 => format!("{} in precompiled chunk", self.why_51()),
            LuaVersion::Lua52 => format!("{} precompiled chunk", self.why_52()),
            LuaVersion::Lua53 => format!("{} precompiled chunk", self.why_53()),
            LuaVersion::Lua54 => return format!("bad binary format ({})", self.why_54()),
            // MacroLua runs on the 5.5 core
            LuaVersion::Lua55 | LuaVersion::MacroLua => {
                return format!("bad binary format ({})", self.why_55());
            }
        };
        match self {
            Bad::Code(d) => format!("{framed} ({d})"),
            _ => framed,
        }
    }

    /// 5.1 compares the whole header in one piece.
    fn why_51(&self) -> String {
        match self {
            Bad::Truncated => "unexpected end".to_string(),
            Bad::IntOverflow => "bad integer".to_string(),
            Bad::Constant => "bad constant".to_string(),
            Bad::Code(_) => "bad code".to_string(),
            _ => "bad header".to_string(),
        }
    }

    fn why_52(&self) -> String {
        match self {
            Bad::Truncated => "truncated".to_string(),
            Bad::NotBinary => "not a".to_string(),
            Bad::Version | Bad::Format => "version mismatch in".to_string(),
            Bad::Size(_) | Bad::NumFormat(_) => "incompatible".to_string(),
            Bad::Corrupted | Bad::IntOverflow | Bad::Constant => "corrupted".to_string(),
            Bad::Code(_) => "corrupted".to_string(),
        }
    }

    fn why_53(&self) -> String {
        match self {
            Bad::Truncated => "truncated".to_string(),
            Bad::NotBinary => "not a".to_string(),
            Bad::Version => "version mismatch in".to_string(),
            Bad::Format => "format mismatch in".to_string(),
            Bad::Size(n) => format!("{} size mismatch in", c_name(*n)),
            Bad::NumFormat(Num::Number) => "float format mismatch in".to_string(),
            Bad::NumFormat(_) => "endianness mismatch in".to_string(),
            Bad::Corrupted | Bad::IntOverflow | Bad::Constant => "corrupted".to_string(),
            Bad::Code(_) => "corrupted".to_string(),
        }
    }

    fn why_54(&self) -> String {
        match self {
            Bad::Size(n) => format!("{} size mismatch", c_name(*n)),
            Bad::NumFormat(Num::Number) => "float format mismatch".to_string(),
            Bad::NumFormat(_) => "integer format mismatch".to_string(),
            _ => self.why_modern(),
        }
    }

    fn why_55(&self) -> String {
        match self {
            Bad::Size(n) => format!("{} size mismatch", name_55(*n)),
            Bad::NumFormat(n) => format!("{} format mismatch", name_55(*n)),
            _ => self.why_modern(),
        }
    }

    /// The 5.4 / 5.5 wording they share.
    fn why_modern(&self) -> String {
        match self {
            Bad::Truncated => "truncated chunk".to_string(),
            Bad::NotBinary => "not a binary chunk".to_string(),
            Bad::Version => "version mismatch".to_string(),
            Bad::Format => "format mismatch".to_string(),
            Bad::Corrupted => "corrupted chunk".to_string(),
            Bad::IntOverflow => "integer overflow".to_string(),
            Bad::Constant => "invalid constant".to_string(),
            Bad::Code(d) => d.clone(),
            Bad::Size(_) | Bad::NumFormat(_) => unreachable!("worded per dialect"),
        }
    }
}

/// 5.3 / 5.4 `checksize` names the C type.
fn c_name(n: Num) -> &'static str {
    match n {
        Num::Int => "int",
        Num::SizeT => "size_t",
        Num::Instruction => "Instruction",
        Num::Integer => "lua_Integer",
        Num::Number => "lua_Number",
    }
}

/// 5.5 `checknum` names the type in words.
fn name_55(n: Num) -> &'static str {
    match n {
        Num::Int => "int",
        Num::SizeT => "size_t",
        Num::Instruction => "instruction",
        Num::Integer => "Lua integer",
        Num::Number => "Lua number",
    }
}

/// `lundump.c`'s chunk name: `@file` and `=name` lose their first
/// character, and a name starting with the escape byte (the default name of
/// a chunk loaded from a string) reads `binary string`.
pub(super) fn lundump_name(chunkname: &[u8]) -> &[u8] {
    match chunkname.first() {
        Some(b'@' | b'=') => &chunkname[1..],
        Some(0x1b) => b"binary string",
        _ => chunkname,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_without_puc_wording_keeps_its_detail() {
        let bad = Bad::Code("register 9 out of range".to_string());
        let got: Vec<String> = [
            LuaVersion::Lua51,
            LuaVersion::Lua52,
            LuaVersion::Lua53,
            LuaVersion::Lua54,
            LuaVersion::Lua55,
        ]
        .into_iter()
        .map(|v| bad.render(v))
        .collect();
        assert_eq!(
            got,
            [
                "bad code in precompiled chunk (register 9 out of range)",
                "corrupted precompiled chunk (register 9 out of range)",
                "corrupted precompiled chunk (register 9 out of range)",
                "bad binary format (register 9 out of range)",
                "bad binary format (register 9 out of range)",
            ]
        );
    }

    #[test]
    fn chunk_names_follow_lundump() {
        assert_eq!(lundump_name(b"@a/b.lua"), b"a/b.lua");
        assert_eq!(lundump_name(b"=stdin"), b"stdin");
        assert_eq!(lundump_name(b"\x1bLua..."), b"binary string");
        assert_eq!(lundump_name(b"plain"), b"plain");
    }
}
