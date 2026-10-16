//! 5.1 and 5.2 sweep their strings first, a bucket of the string table at
//! a time (`GCSsweepstring`): the dead ones leave the count bucket by
//! bucket, before the sweep of the other objects. luna's strings are on
//! the object list; the ones PUC's string table holds are taken off the
//! count here, by PUC's bucket, and freed without counting when the
//! object sweep reaches them.

use super::*;
use crate::runtime::table::Dialect;

/// In a string's `sub` byte: the string sweep took it off the count; the
/// object sweep frees it without counting.
pub(crate) const SWEPT: u8 = 0x10;

/// The dead strings of PUC's string table by bucket, the next one the
/// string sweep reaches, and those taken off the object list before it.
#[derive(Default)]
pub(super) struct StrtSweep {
    dead: Vec<(u32, *mut GcHeader)>,
    next: usize,
    pub(super) held: Vec<*mut GcHeader>,
}

impl Heap {
    /// The atomic step ended (5.1, 5.2): the dead strings of PUC's string
    /// table, by bucket.
    pub(super) fn collect_dead_strings(&mut self) {
        self.strt.dead.clear();
        self.strt.next = 0;
        if !matches!(self.table_dialect, Dialect::L51 | Dialect::L52) {
            return;
        }
        let size = self.puc.strt_size as u32;
        let new_white = self.current_white;
        let mut cur = self.all;
        // SAFETY: the links of `all` lead to live objects; no object is
        // freed while the list is walked
        unsafe {
            while !cur.is_null() {
                let f = (*cur).flags;
                if (*cur).tag == ObjTag::Str
                    && is_white(f)
                    && f & new_white == 0
                    && !is_uncounted(cur)
                    && (*cur).sub & EXTERNAL == 0
                {
                    let s = &*(cur as *const LuaStr);
                    if self.table_dialect == Dialect::L51 || s.is_short() {
                        let b = string::bytes_of(cur as *const LuaStr);
                        let h = match self.table_dialect {
                            Dialect::L51 => string::lua_hash_51(b),
                            _ => hash_52(b, self.seed),
                        };
                        self.strt.dead.push((h % size.max(1), cur));
                    }
                }
                cur = (*cur).next;
            }
        }
        // a stable sort keeps a bucket's strings in the order of the list
        self.strt.dead.sort_by_key(|e| e.0);
    }

    /// Whether `h`, dead, waits for the string sweep under way (every dead
    /// string of PUC's string table does, until it is over).
    pub(super) fn in_string_sweep(&self, h: *mut GcHeader) -> bool {
        // SAFETY: `h` heads a live object of this heap
        unsafe {
            self.strt.next < self.strt.dead.len()
                && (*h).tag == ObjTag::Str
                && (*h).sub & (UNCOUNTED | EXTERNAL | SWEPT) == 0
                && (self.table_dialect == Dialect::L51 || (*(h as *const LuaStr)).is_short())
        }
    }

    /// The string sweep is over (or a full collection ends it): the strings
    /// held off the object list are freed, or put back when a program made
    /// them again meanwhile.
    pub(crate) fn end_string_sweep(&mut self) {
        let new_white = self.current_white;
        for h in std::mem::take(&mut self.strt.held) {
            // SAFETY: a held string is allocated and on no list
            unsafe {
                let f = (*h).flags;
                if is_white(f) && f & new_white == 0 {
                    self.free_obj(h);
                    self.live -= 1;
                } else {
                    (*h).next = self.all;
                    self.all = h;
                }
            }
        }
        self.strt.dead.clear();
        self.strt.next = 0;
    }

    /// The string sweep went up to bucket `upto`: the dead strings of the
    /// buckets before it leave the count. Returns the bytes freed.
    pub(crate) fn sweep_strings_upto(&mut self, upto: i64) -> i64 {
        let before = self.bytes as i64;
        while let Some(&(b, h)) = self.strt.dead.get(self.strt.next) {
            if i64::from(b) >= upto {
                break;
            }
            self.strt.next += 1;
            // SAFETY: a dead string stays allocated until the object sweep,
            // which comes after this one; a resurrected one is skipped
            unsafe {
                let f = (*h).flags;
                if !is_white(f) || f & self.current_white != 0 {
                    continue;
                }
                (*h).sub |= SWEPT;
                self.acct_string_free((*(h as *const LuaStr)).len());
            }
        }
        before - self.bytes as i64
    }
}

/// PUC 5.2's `luaS_hash`: from the last byte, every `len / 32 + 1`-th.
fn hash_52(bytes: &[u8], seed: u32) -> u32 {
    let l = bytes.len();
    let mut h = seed ^ l as u32;
    let step = (l >> 5) + 1;
    let mut l1 = l;
    while l1 >= step {
        h ^= h
            .wrapping_shl(5)
            .wrapping_add(h >> 2)
            .wrapping_add(u32::from(bytes[l1 - 1]));
        l1 -= step;
    }
    h
}

impl Heap {
    /// `gc-verify`, once marking is done (the last sweep over): PUC's
    /// string table holds a place for each counted short string luna has
    /// and each one a record holds. Panics when the count went astray.
    #[cfg(feature = "gc-verify")]
    pub(super) fn verify_string_count(&self, ctx: &str) {
        // and "not enough memory", which PUC makes at the start and luna
        // does not
        let mut n = self.ghost_short_strings() + 1;
        for mut cur in [self.all, self.fixed] {
            // SAFETY: the lists link this heap's live objects
            unsafe {
                while !cur.is_null() {
                    if (*cur).tag == ObjTag::Str
                        && (*(cur as *const LuaStr)).len() <= self.puc.short_max
                        && (*cur).sub & (UNCOUNTED | EXTERNAL | SWEPT) == 0
                    {
                        n += 1;
                    }
                    cur = (*cur).next;
                }
            }
        }
        assert_eq!(
            n, self.puc.strt_nuse,
            "{ctx}: counted short strings against PUC's string table"
        );
    }
}
