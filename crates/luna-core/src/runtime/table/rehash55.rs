//! PUC 5.5's rehash sizing, which differs from 5.1–5.4's: the array part
//! may be up to three times larger than its element count (a hash node
//! costs about three array slots), a rehash whose keys all stay in the
//! hash part keeps the array part as it is, and a hash part that had
//! removed entries, or none at all, gets a quarter more room.

use super::*;

/// PUC 5.5 `MAXASIZE` for its `arrayindex`: a larger key is never counted
/// as an array index.
const ARRAY_INDEX_LIMIT: u64 = 1 << 31;

impl Table {
    pub(super) fn rehash_55(&mut self, heap: &mut Heap, pending: Value) -> Result<(), TableError> {
        let mut nums = [0usize; 33];
        let mut na = 0usize;
        let mut total = 1usize;
        // the shared empty hash part (PUC `dummynode`) is one node with no
        // value, which PUC's count takes for a removed entry
        let mut deleted = self.nodes().is_empty();
        let count = |k: Value, nums: &mut [usize; 33], na: &mut usize| {
            if let Value::Int(i) = k
                && (i as u64).wrapping_sub(1) < ARRAY_INDEX_LIMIT
            {
                nums[ceil_log2(i as u64)] += 1;
                *na += 1;
            }
        };
        count(pending, &mut nums, &mut na);
        for n in self.nodes().iter() {
            if n.val.is_nil() {
                deleted = true;
            } else {
                total += 1;
                count(n.key(), &mut nums, &mut na);
            }
        }
        let new_asize = if na == 0 {
            self.asize()
        } else {
            for (i, &tag) in self.atags().iter().enumerate() {
                if tag != raw::NIL {
                    nums[ceil_log2(i as u64 + 1)] += 1;
                    na += 1;
                    total += 1;
                }
            }
            let (optimal, in_array) = sizes::computesizes_55(&nums, na);
            na = in_array;
            optimal
        };
        if new_asize > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        let mut nsize = total - na;
        if deleted {
            nsize += nsize >> 2;
        }
        if nsize > MAX_ASIZE {
            return Err(TableError::Overflow);
        }
        self.resize(heap, new_asize, nsize)
    }
}
