//! luna's behaviour in each dialect where the runtime differs: the rules a
//! table follows, how strings hash, what the collector keeps and when it
//! finalizes, where PUC checks for a collection. Each difference is one
//! entry of [`Rules`], decided here once per dialect.

use crate::version::LuaVersion;

/// The PUC version whose rules apply. A table keeps it in its header byte
/// (how its parts are sized, where a number key goes in the hash part,
/// which border `#t` returns).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub(crate) enum Dialect {
    L51 = 1,
    L52,
    L53,
    L54,
    L55,
}

impl Dialect {
    pub(crate) const fn of(v: LuaVersion) -> Dialect {
        match v {
            LuaVersion::Lua51 => Dialect::L51,
            LuaVersion::Lua52 => Dialect::L52,
            LuaVersion::Lua53 => Dialect::L53,
            LuaVersion::Lua54 | LuaVersion::MacroLua => Dialect::L54,
            LuaVersion::Lua55 => Dialect::L55,
        }
    }
}

/// The rules of one dialect.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Rules {
    dialect: Dialect,
    ephemeron: bool,
    refinalize: bool,
    signed_zero_keys: bool,
    hash51: bool,
}

impl Rules {
    pub(crate) const fn of(v: LuaVersion) -> Rules {
        Rules::for_dialect(Dialect::of(v))
    }

    pub(crate) const fn for_dialect(d: Dialect) -> Rules {
        Rules {
            dialect: d,
            // PUC 5.1 has no ephemeron pass: a `__mode='k'` table marks its
            // values strongly (5.1's gc.lua "weak tables" relies on that)
            ephemeron: !matches!(d, Dialect::L51),
            // 5.2's `luaC_checkfinalizer` refuses an object finalized once
            refinalize: !matches!(d, Dialect::L52),
            // 5.1 and 5.2: a new table key -0 stays -0
            signed_zero_keys: matches!(d, Dialect::L51 | Dialect::L52),
            hash51: matches!(d, Dialect::L51),
            // 5.3 finalizes a table in a cycle through an unreachable
            // coroutine one collection after it finds it unreachable
            // (gc.lua :502); 5.4 and 5.5 in the same one (gc.lua :544)
        }
    }

    #[inline]
    pub(crate) fn dialect(self) -> Dialect {
        self.dialect
    }

    /// A weak-keyed table's values are marked through its keys (not 5.1).
    #[inline]
    pub(crate) fn ephemeron(self) -> bool {
        self.ephemeron
    }

    /// An object finalized once can be registered again (not 5.2).
    #[inline]
    pub(crate) fn refinalize(self) -> bool {
        self.refinalize
    }

    #[inline]
    pub(crate) fn signed_zero_keys(self) -> bool {
        self.signed_zero_keys
    }

    /// Strings hash with PUC 5.1's function (`string::lua_hash_51`).
    #[inline]
    pub(crate) fn hash51(self) -> bool {
        self.hash51
    }

    /// 5.2 checks for a collection on every call (`luaD_precall`).
    #[inline]
    pub(crate) fn check_on_call(self) -> bool {
        matches!(self.dialect, Dialect::L52)
    }

    /// 5.1 checks when a call from C returns (`luaD_call`).
    #[inline]
    pub(crate) fn check_on_return(self) -> bool {
        matches!(self.dialect, Dialect::L51)
    }

    /// 5.1 and 5.2: the C API checks before it allocates; later ones after
    /// it pushed what it made.
    #[inline]
    pub(crate) fn api_checks_before(self) -> bool {
        self.dialect <= Dialect::L52
    }
}
