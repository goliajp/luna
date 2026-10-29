//! 5.1 and 5.2 sweep their strings first, a bucket of the string table at
//! a time (`GCSsweepstring`): the dead ones leave the count bucket by
//! bucket, before the sweep of the other objects. luna's strings are on
//! the object list; the ones PUC's string table holds are taken off the
//! count here, by PUC's bucket, and freed without counting when the
//! object sweep reaches them.

use super::*;
use crate::runtime::dialect::Dialect;

mod order;

/// In a string's `sub` byte: the string sweep took it off the count; the
/// object sweep frees it without counting.
pub(crate) const SWEPT: u8 = 0x10;

/// The dead strings of PUC's string table by bucket, the next one the
/// string sweep reaches, and those taken off the object list before it.
#[derive(Default)]
pub(in crate::runtime) struct StrtSweep {
    /// the dead strings found, with their buckets
    found: Vec<(u32, *mut GcHeader)>,
    /// `found` by bucket in the first `len` places; the buffer only grows,
    /// so a collection writes its places without clearing them first
    dead: Vec<(u32, *mut GcHeader)>,
    len: usize,
    next: usize,
    pub(in crate::runtime) held: Vec<*mut GcHeader>,
    counts: Vec<u32>,
    /// the place the newest string of each bucket has in its chain
    seq: Vec<u16>,
    /// the table grew from this many buckets for a string that has not
    /// entered yet (`Heap::strt_enter`)
    pub(super) grow_from: Option<usize>,
    /// room for renumbering the chains after a resize
    order: Vec<(u32, u16, Gc<LuaStr>)>,
    /// the old `seq` kept for the next resize
    spare: Vec<u16>,
    ordered: Vec<(u32, u16, Gc<LuaStr>)>,
}

impl StrtSweep {
    /// `dead` in bucket order, a bucket's strings in the order found: by
    /// counting when the table is not much bigger than the list. Every
    /// entry's bucket is below `buckets`.
    #[inline(never)]
    fn sort_dead(&mut self, buckets: u32) {
        let n = self.found.len();
        self.len = n;
        // every place below `n` is written; a power of two long, so that a
        // place masked with it is seen to be within it
        let pm = n.next_power_of_two() - 1;
        if self.dead.len() <= pm {
            self.dead.resize(pm + 1, (0, std::ptr::null_mut()));
        }
        let dead = &mut self.dead[..=pm];
        if buckets as usize > 8 * n {
            self.found.sort_by_key(|e| e.0);
            dead[..n].copy_from_slice(&self.found);
            return;
        }
        // a power of two: masking a bucket keeps it in `counts`, which the
        // compiler then sees without a bounds check
        let m = buckets.max(1) - 1;
        debug_assert!(buckets <= 1 || buckets.is_power_of_two());
        self.counts.clear();
        self.counts.resize(m as usize + 1, 0);
        let counts = &mut self.counts[..=m as usize];
        for &(b, _) in &self.found {
            debug_assert!(b <= m);
            counts[(b & m) as usize] += 1;
        }
        // each bucket's first place
        let mut at = 0;
        for c in counts.iter_mut() {
            (*c, at) = (at, at + *c);
        }
        for &(b, p) in &self.found {
            // the bucket is below `buckets`: masked, it is the same
            let b = b & m;
            let at = &mut counts[b as usize];
            dead[*at as usize & pm] = (b, p);
            *at += 1;
        }
    }
}

impl Heap {
    /// The atomic step ended (5.1, 5.2): the dead strings of PUC's string
    /// table, by bucket.
    pub(in crate::runtime) fn collect_dead_strings(&mut self) {
        self.puc.records.strt.found.clear();
        self.puc.records.strt.len = 0;
        self.puc.records.strt.next = 0;
        if !matches!(self.puc.dialect, Dialect::L51 | Dialect::L52) {
            return;
        }
        let size = self.puc.records.strt_size as u32;
        // PUC's table sizes are powers of two
        debug_assert!(size <= 1 || size.is_power_of_two());
        let mask = size.max(1) - 1;
        let new_white = self.white();
        let mut cur = self.list_head();
        // SAFETY: the links of `all` lead to live objects; no object is
        // freed while the list is walked
        unsafe {
            while !cur.is_null() {
                let f = (*cur).flags();
                if (*cur).tag == ObjTag::Str
                    && is_white(f)
                    && f & new_white == 0
                    && !(*cur).is_uncounted()
                    && (*cur).sub & EXTERNAL == 0
                {
                    let s = &*(cur as *const LuaStr);
                    if self.puc.dialect == Dialect::L51 || s.is_short() {
                        let b = string::bytes_of(cur as *const LuaStr);
                        // the string's own hash is PUC's: 5.1's, and 5.2's
                        // below 32 bytes (where it takes every byte)
                        let h = match self.puc.dialect {
                            Dialect::L51 => Gc::from_ptr(cur as *mut LuaStr).stored_hash(),
                            _ if b.len() < 32 => Gc::from_ptr(cur as *mut LuaStr).stored_hash(),
                            _ => hash_52(b, self.seed()),
                        };
                        self.puc.records.strt.found.push((h & mask, cur));
                    }
                }
                cur = (*cur).next_obj();
            }
        }
        self.puc.records.strt.sort_dead(size.max(1));
        // each bucket's from the head of its chain (the trace's order)
        if !crate::runtime::gc_hooks::TRACE {
            return;
        }
        let strt = &mut self.puc.records.strt;
        let dead = &mut strt.dead[..strt.len];
        let mut i = 0;
        while i < dead.len() {
            let b = dead[i].0;
            let mut j = i + 1;
            while j < dead.len() && dead[j].0 == b {
                j += 1;
            }
            if j - i > 1 {
                dead[i..j].sort_unstable_by_key(|e| {
                    // SAFETY: the dead strings are allocated
                    std::cmp::Reverse(unsafe { Gc::from_ptr(e.1 as *mut LuaStr).chain() })
                });
            }
            i = j;
        }
    }

    /// Whether `h`, dead, waits for the string sweep under way (every dead
    /// string of PUC's string table does, until it is over).
    ///
    /// # Safety
    /// `h` heads an object of this heap that is still allocated.
    pub(in crate::runtime::puc) unsafe fn in_string_sweep(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            self.puc.records.strt.next < self.puc.records.strt.len
                && (*h).tag == ObjTag::Str
                && (*h).sub & (UNCOUNTED | EXTERNAL | SWEPT) == 0
                && (self.puc.dialect == Dialect::L51 || (*(h as *const LuaStr)).is_short())
        }
    }

    /// The list sweep reached dead string `h` that PUC's string sweep, not
    /// the list's, frees: held off the list until that sweep ends.
    #[inline]
    pub(in crate::runtime::puc) fn hold_for_string_sweep(&mut self, h: *mut GcHeader) {
        self.puc.records.strt.held.push(h);
    }

    /// The strings held for the string sweep, taken (the heap is dropped).
    pub(in crate::runtime::puc) fn take_held_strings(&mut self) -> Vec<*mut GcHeader> {
        std::mem::take(&mut self.puc.records.strt.held)
    }

    /// The string sweep is over (or a full collection ends it): the strings
    /// held off the object list are freed, or put back when a program made
    /// them again meanwhile.
    pub(in crate::runtime) fn end_string_sweep(&mut self) {
        let new_white = self.white();
        for h in std::mem::take(&mut self.puc.records.strt.held) {
            // SAFETY: a held string is allocated and on no list
            unsafe {
                let f = (*h).flags();
                if is_white(f) && f & new_white == 0 && !self.kept_dead(h) {
                    self.free_unlisted(h);
                } else {
                    self.relink(h);
                }
            }
        }
        self.puc.records.strt.len = 0;
        self.puc.records.strt.next = 0;
    }

    /// The string sweep went up to bucket `upto`: the dead strings of the
    /// buckets before it leave the count. Returns the bytes freed.
    pub(in crate::runtime::puc) fn sweep_strings_upto(&mut self, upto: i64) -> i64 {
        let before = self.puc.count.bytes as i64;
        let new_white = self.white();
        // taken for the walk, so that it is not read again each step
        let s = &mut self.puc.records.strt;
        let dead = std::mem::take(&mut s.dead);
        let mut next = s.next;
        for &(b, h) in &dead[next..s.len] {
            if i64::from(b) >= upto {
                break;
            }
            next += 1;
            // SAFETY: a dead string stays allocated until the object sweep,
            // which comes after this one; a resurrected one is skipped
            unsafe {
                let f = (*h).flags();
                if !is_white(f) || f & new_white != 0 {
                    continue;
                }
                (*h).sub |= SWEPT;
                self.hook_string_swept(Gc::from_ptr_unchecked(h));
                self.acct_string_free((*(h as *const LuaStr)).len());
            }
        }
        self.puc.records.strt.next = next;
        self.puc.records.strt.dead = dead;
        before - self.puc.count.bytes as i64
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

impl Heap {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_by_bucket_keeps_the_order_a_stable_sort_gives() {
        let mut seed = 7u32;
        for (n, buckets) in [(0, 4), (1, 1), (50, 8), (200, 64), (40, 1024), (300, 512)] {
            let dead: Vec<(u32, *mut GcHeader)> = (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                    (
                        (seed >> 8) % buckets,
                        std::ptr::without_provenance_mut(i * 16 + 16),
                    )
                })
                .collect();
            let mut want = dead.clone();
            want.sort_by_key(|e| e.0);
            let mut s = StrtSweep {
                found: dead,
                ..Default::default()
            };
            s.sort_dead(buckets);
            assert_eq!(s.dead[..s.len], want, "{n} in {buckets}");
        }
    }
}
