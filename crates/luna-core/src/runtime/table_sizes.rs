//! How PUC sizes a table's parts when it rehashes, from how many integer
//! keys fall in each slice `(2^(b-1), 2^b]` of the array (`nums[b]`): for
//! luna's tables and for the model's records of the parser's tables.

/// For k ≥ 1: the slice l with k ∈ (2^(l-1), 2^l] (PUC `luaO_ceillog2`).
#[inline]
pub(crate) fn ceil_log2(k: u64) -> usize {
    (u64::BITS - (k - 1).leading_zeros()) as usize
}

/// 5.1–5.4 `computesizes`, `int_keys` integer keys in all: the largest
/// power of two with more than half its slots taken by keys, and how many
/// keys go there.
pub(crate) fn computesizes(nums: &[usize], int_keys: usize) -> (usize, usize) {
    let (mut a, mut asize, mut in_array) = (0usize, 0usize, 0usize);
    let mut two_to_i = 1usize;
    let mut i = 0;
    while int_keys > two_to_i / 2 && i < nums.len() {
        a += nums[i];
        if a > two_to_i / 2 {
            asize = two_to_i;
            in_array = a;
        }
        i += 1;
        match two_to_i.checked_mul(2) {
            Some(n) => two_to_i = n,
            None => break,
        }
    }
    (asize, in_array)
}

/// 5.5 `computesizes`, `na` integer keys in all: the largest power of two
/// that holds more keys in its last slice and no more than three times as
/// many slots as keys up to it, and how many keys go there.
pub(crate) fn computesizes_55(nums: &[usize], na: usize) -> (usize, usize) {
    let (mut a, mut optimal, mut in_array) = (0usize, 0usize, 0usize);
    let mut twotoi = 1usize;
    for &n in nums.iter() {
        if twotoi > 3 * na {
            break;
        }
        a += n;
        if n > 0 && twotoi <= 3 * a {
            optimal = twotoi;
            in_array = a;
        }
        twotoi *= 2;
    }
    (optimal, in_array)
}
