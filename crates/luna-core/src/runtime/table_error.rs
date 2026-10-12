//! What a table mutation can fail with.

/// Errors that table mutation can raise back to the interpreter.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableError {
    /// `t[nil] = …` — `nil` is forbidden as a key.
    NilIndex,
    /// `t[0/0] = …` — NaN floats are forbidden as keys.
    NanIndex,
    /// `next` called with a key not present in the table.
    InvalidNext,
    /// PUC `luaH_resizearray` — the array part would have to grow past
    /// `MAXASIZE`, or the hash part past `MAXHBITS`. Raised back as
    /// "table overflow" so a runaway `a[i] = i` loop walls within budget
    /// (5.5/5.4 heavy.lua's `toomanyidx` pcalls exactly this scenario).
    Overflow,
    /// The table is read-only (see [`Table::is_readonly`](super::Table::is_readonly)); nothing was
    /// written. Raised back as "Attempt to modify a readonly table".
    ReadOnly,
    /// The table needed to grow and the allocation failed; nothing was
    /// written. Raised back as a memory error.
    Memory(crate::runtime::mem::Oom),
}

impl From<crate::runtime::mem::Oom> for TableError {
    fn from(o: crate::runtime::mem::Oom) -> TableError {
        TableError::Memory(o)
    }
}
