//! Lua dialect selection.
//!
//! Grammar and semantics switches are expressed as capability predicates, not
//! version comparisons at use sites, so further dialects can be added by
//! extending this enum only.

/// Lua dialect the VM emulates. Drives lexer, parser, and runtime feature
/// gating. `Lua55` is the primary; the others are compat modes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum LuaVersion {
    /// Lua 5.1 — no integer subtype, `arg` table, no `goto`.
    Lua51,
    /// Lua 5.2 — adds `goto` / `bit32` / `\xXX` escapes, retires `setfenv`.
    Lua52,
    /// Lua 5.3 — adds native 64-bit integers, bitwise ops, `string.pack`.
    Lua53,
    /// Lua 5.4 — adds `<const>` / `<close>` attributes and integer-for spec.
    Lua54,
    /// Lua 5.5 — adds `global` declarations and named vararg parameters.
    Lua55,
}

impl LuaVersion {
    /// Integer subtype and integer literals (5.3+).
    pub fn has_integers(self) -> bool {
        self >= LuaVersion::Lua53
    }

    /// `goto` / `::label::` (5.2+); `goto` is a reserved word.
    pub fn has_goto(self) -> bool {
        self >= LuaVersion::Lua52
    }

    /// `& | ~ << >>` operators (5.3+).
    pub fn has_bitwise_ops(self) -> bool {
        self >= LuaVersion::Lua53
    }

    /// `//` floor division (5.3+).
    pub fn has_idiv(self) -> bool {
        self >= LuaVersion::Lua53
    }

    /// `<const>` / `<close>` attributes on local declarations (5.4+).
    pub fn has_attribs(self) -> bool {
        self >= LuaVersion::Lua54
    }

    /// Hexadecimal float literals `0x1p4` (5.2+).
    pub fn has_hex_float(self) -> bool {
        self >= LuaVersion::Lua52
    }

    /// String escapes `\z`, `\xXX` (5.2+) and `\u{XXX}` (5.3+).
    pub fn has_extended_escapes(self) -> bool {
        self >= LuaVersion::Lua52
    }

    /// `\u{XXX}` unicode escape specifically (5.3+).
    pub fn has_unicode_escape(self) -> bool {
        self >= LuaVersion::Lua53
    }

    /// Empty statement `;` (5.2+).
    pub fn has_empty_statement(self) -> bool {
        self >= LuaVersion::Lua52
    }

    /// `global` declarations; `global` is a reserved word (5.5+).
    pub fn has_global_decl(self) -> bool {
        self >= LuaVersion::Lua55
    }

    /// Named vararg parameter `function f(...name)` (5.5+).
    pub fn has_named_vararg(self) -> bool {
        self >= LuaVersion::Lua55
    }

    /// Leading collective attribute in declarations: `local <const> a, b` (5.5+).
    pub fn has_collective_attrib(self) -> bool {
        self >= LuaVersion::Lua55
    }

    /// In 5.1 `break` is a "last statement" like `return`; later versions allow
    /// it anywhere in a block.
    pub fn break_is_last_statement(self) -> bool {
        self == LuaVersion::Lua51
    }

    /// In 5.1, `[[` inside a level-0 long string is an error
    /// ("nesting of [[...]] is deprecated").
    pub fn rejects_nested_long_string(self) -> bool {
        self == LuaVersion::Lua51
    }
}
