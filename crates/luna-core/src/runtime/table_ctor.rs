//! A table constructor's size hints (PUC `NEWTABLE`'s operands) and the
//! array growth of its list stores (`SETLIST`): together they decide the
//! array part's size, and so which border `#t` finds in it.

use super::*;

/// The largest array size `NewTable`'s `C` operand carries in 5.4 / 5.5. A
/// constructor with more list items reaches its size anyway: its last
/// `SetList` grows the array part to exactly the item count, as PUC's
/// would have been from the start, and resets the length state the same.
const MAX_MODERN_HINT: u32 = 0xFF;

/// PUC `luaO_int2fb`: the "floating point byte" 5.1–5.3 code a size in
/// (`eeeeexxx`, rounded up).
pub(crate) fn int2fb(mut x: u32) -> u32 {
    let mut e = 0;
    while x >= 16 {
        x = x.div_ceil(2);
        e += 1;
    }
    if x < 8 { x } else { ((e + 1) << 3) | (x - 8) }
}

/// PUC `luaO_fb2int`.
pub(crate) fn fb2int(x: u32) -> usize {
    let e = (x >> 3) & 0x1f;
    if e == 0 {
        x as usize
    } else {
        (((x & 7) + 8) as usize) << (e - 1)
    }
}

/// `NewTable`'s `(B, C, k)` for a constructor with `na` list items (not
/// counting a last one that is a call or `...`) and `nh` keyed fields, as
/// the dialect's `luac` sizes it. With `k` set (5.1–5.3), `B` and `C` are
/// the two sizes as floating point bytes; without (5.4 / 5.5), `B` is log2
/// of the hash size plus one and `C` the array size. The form travels with
/// the op, so a chunk keeps its sizes in a state of another dialect.
pub(crate) fn new_table_operands(
    v: crate::version::LuaVersion,
    na: u32,
    nh: u32,
) -> (u32, u32, bool) {
    if Dialect::of(v) <= Dialect::L53 {
        (int2fb(na), int2fb(nh), true)
    } else {
        let b = if nh == 0 {
            0
        } else {
            ceil_log2(u64::from(nh)) as u32 + 1
        };
        (b, na.min(MAX_MODERN_HINT), false)
    }
}

/// `(array size, hash size)` a `NewTable` with these operands makes (see
/// [`new_table_operands`]); `None` past the sizes a table can have, which
/// only crafted bytecode asks for. Public for the JIT crates.
#[doc(hidden)]
pub fn new_table_sizes(b: u32, c: u32, k: bool) -> Option<(usize, usize)> {
    let (asize, hsize) = if k {
        (fb2int(b), fb2int(c))
    } else {
        (
            c as usize,
            if b == 0 {
                0
            } else {
                1usize.checked_shl(b - 1)?
            },
        )
    };
    (asize <= MAX_ASIZE && hsize <= MAX_ASIZE).then_some((asize, hsize))
}

impl Table {
    /// Before a `SetList` stores up to index `last`: grow the array part to
    /// exactly `last` when it is smaller, keeping the hash part's size
    /// (PUC `luaH_resizearray`). Public for the JIT crates.
    #[doc(hidden)]
    pub fn reserve_list(&mut self, heap: &mut Heap, last: u64) -> Result<(), TableError> {
        if last <= self.asize {
            return Ok(());
        }
        if last > MAX_ASIZE as u64 {
            return Err(TableError::Overflow);
        }
        let nodes = self.nodes().len();
        self.resize(heap, last as usize, nodes);
        Ok(())
    }

    /// Store list item `idx` (0-based, below `asize`) the way `SetList`
    /// does: straight into the array slot, nil included.
    #[inline]
    pub(crate) fn set_list_slot(&mut self, idx: usize, v: Value) {
        debug_assert!(idx < self.asize());
        self.aset(idx, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_point_bytes_round_up() {
        for (n, back) in [
            (0, 0),
            (7, 7),
            (8, 8),
            (15, 15),
            (16, 16),
            (17, 18),
            (300, 320),
        ] {
            assert_eq!(fb2int(int2fb(n)), back, "{n}");
        }
    }
}
