//! The hash-part entry of a table, and the slot-state word of the SoA
//! hash part.

use crate::runtime::heap::Gc;
use crate::runtime::value::Value;

/// A hash-part entry (PUC `Node`): the key is kept as its tag and its
/// payload at the offsets a `Value` has them, so the first 16 bytes read
/// as the key ([`Node::key`]), and `dead_key` and `next` live in what is a
/// `Value`'s padding (PUC `NodeKey`). 32 bytes, so a chain step is a shift.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Node {
    pub(super) key_tag: u8,
    /// PUC `setdeadkey` analogue: the key was a collectable that got swept
    /// out of a weak table; the key is now nil, and this flag tells
    /// `insert_new` that the slot still sits in a chain (its `next` is
    /// kept) rather than being free.
    pub(super) dead_key: bool,
    /// the key is integer 0 but was stored as -0 under 5.1/5.2, which
    /// keep a new key as it was given; iteration hands it back that way
    pub(super) neg_zero: bool,
    /// absolute index of the next node in this chain, or NONE
    pub(super) next: i32,
    pub(super) key_payload: std::mem::MaybeUninit<u64>,
    pub(super) val: Value,
}

pub(super) const NONE: i32 = -1;

impl Node {
    pub(super) const EMPTY: Node = Node {
        key_tag: crate::runtime::value::tag::NIL,
        dead_key: false,
        neg_zero: false,
        next: NONE,
        key_payload: std::mem::MaybeUninit::uninit(),
        val: Value::Nil,
    };

    /// A live entry.
    #[inline]
    pub(super) fn new(key: Value, val: Value, next: i32) -> Node {
        let mut n = Node {
            val,
            next,
            ..Node::EMPTY
        };
        n.set_key(key);
        n
    }

    /// The key.
    #[inline(always)]
    pub(super) fn key(&self) -> Value {
        // SAFETY: `#[repr(C)]` puts `key_tag` and `key_payload` where a
        // `Value` has its tag and payload, and `set_key` stores them as
        // a `Value` has them; the bytes between are a `Value`'s padding
        unsafe { *(self as *const Node as *const Value) }
    }

    /// The key as iteration hands it back.
    #[inline]
    pub(super) fn shown_key(&self) -> Value {
        if self.neg_zero {
            Value::Float(-0.0)
        } else {
            self.key()
        }
    }

    /// Store `key`, leaving `dead_key` and `next` as they are.
    #[inline(always)]
    pub(super) fn set_key(&mut self, key: Value) {
        let src = &key as *const Value as *const u8;
        // SAFETY: a `Value`'s tag is its first byte and its payload (which
        // may be padding, hence `MaybeUninit`) its second word
        unsafe {
            self.key_tag = *src;
            self.key_payload = *(src.add(8) as *const std::mem::MaybeUninit<u64>);
        }
    }

    /// True when the key is the string `key` (the same object).
    #[inline(always)]
    pub(super) fn key_is_str(&self, key: Gc<crate::runtime::string::LuaStr>) -> bool {
        self.key_tag == crate::runtime::value::tag::STR
            // SAFETY: a string key's payload is its pointer
            && unsafe { self.key_payload.assume_init() } == key.as_ptr() as usize as u64
    }

    /// True when the slot holds no key and sits in no chain.
    #[inline(always)]
    pub(super) fn is_free(&self) -> bool {
        self.key_tag == crate::runtime::value::tag::NIL && !self.dead_key
    }
}

/// SoA Robin Hood meta-word layout.
///
/// Each `meta[idx]` slot encodes the open-addressing slot state in a
/// single u16:
/// - bit 15 (`OCCUPIED_BIT`): 0 = empty, 1 = occupied
/// - bit 14 (`TOMBSTONE_BIT`): 0 = live, 1 = tombstoned-occupied
/// - bits 13..0 (`PSL_MASK`): probe-sequence length (0..16383)
///
/// The 14-bit PSL field is **far** beyond any realistic Robin Hood
/// max-PSL at load ≤ 0.75 (expected max ~20 on 1024 slots; even the
/// long-tail outliers seen empirically with luna's existing hash
/// distributions stay under 200). 2 bytes/slot is still 20× smaller
/// than the 40-byte Node, so the SoA bandwidth gain is preserved.
///
/// A 1-byte meta with a 6-bit PSL cap of 63 is too narrow: under load
/// 0.676 on cap=1024 the LuaStr+mix64 hash distribution produces a
/// long-tail PSL of 64+.
///
/// Tombstones do NOT free the slot for `find` (probe continues past), but
/// DO free it for `insert` (write the new entry, clear the tomb bit). They
/// accumulate; the rehash path compacts them periodically.
#[allow(dead_code)]
pub(crate) mod meta_bits {
    pub const OCCUPIED_BIT: u16 = 0b1000_0000_0000_0000;
    pub const TOMBSTONE_BIT: u16 = 0b0100_0000_0000_0000;
    pub const PSL_MASK: u16 = 0b0011_1111_1111_1111;
    pub const PSL_MAX: u16 = PSL_MASK;
    /// Empty slot — bit 15 = 0, all others 0.
    pub const EMPTY: u16 = 0;

    #[inline(always)]
    pub fn is_occupied(m: u16) -> bool {
        (m & OCCUPIED_BIT) != 0
    }
    #[inline(always)]
    pub fn is_tombstone(m: u16) -> bool {
        (m & TOMBSTONE_BIT) != 0
    }
    /// Live = occupied AND not tombstoned. `next()` iteration cursor returns
    /// these. `find_slot_rh` short-circuits on a live match.
    #[inline(always)]
    pub fn is_live(m: u16) -> bool {
        (m & (OCCUPIED_BIT | TOMBSTONE_BIT)) == OCCUPIED_BIT
    }
    #[inline(always)]
    pub fn psl(m: u16) -> u16 {
        m & PSL_MASK
    }
    #[inline(always)]
    pub fn pack(psl: u16, tomb: bool) -> u16 {
        debug_assert!(psl <= PSL_MAX);
        let mut m = OCCUPIED_BIT | (psl & PSL_MASK);
        if tomb {
            m |= TOMBSTONE_BIT;
        }
        m
    }
}

impl super::Table {
    /// The node holding the string key `key`, found by pointer (PUC
    /// `luaH_getshortstr`). Exact for a short (interned) string; for a long
    /// one a hit is exact and a miss proves nothing.
    #[inline(always)]
    fn str_node_by_ptr(&self, key: Gc<crate::runtime::string::LuaStr>) -> Option<usize> {
        #[cfg(feature = "gc-verify")]
        self.verify_find_node_keys(Value::Str(key));
        let mask = self.node_mask;
        if mask >> 63 != 0 {
            return None;
        }
        debug_assert_eq!(mask as usize + 1, self.nodes.len());
        // a short string's hash is set when it is interned; a long one's
        // may still be the seed, which only makes a hit unlikely
        let mut idx = (u64::from(key.stored_hash()) & mask) as usize;
        loop {
            // SAFETY: the main position is masked to the node count and
            // every `next` link is a node index written by `insert_new`
            let node = unsafe { self.nodes.get_unchecked(idx) };
            if node.key_is_str(key) {
                return Some(idx);
            }
            if node.next == NONE {
                return None;
            }
            idx = node.next as usize;
        }
    }

    /// The value slot of string key `key`; see [`Self::str_node_by_ptr`].
    #[inline(always)]
    pub(crate) fn str_slot_by_ptr(
        &self,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<&Value> {
        let i = self.str_node_by_ptr(key)?;
        // SAFETY: a node index found above
        Some(unsafe { &self.nodes.get_unchecked(i).val })
    }

    /// [`Self::str_slot_by_ptr`] for a write.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(dead_code))]
    pub(crate) fn str_slot_by_ptr_mut(
        &mut self,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<&mut Value> {
        let i = self.str_node_by_ptr(key)?;
        // SAFETY: a node index found above
        Some(unsafe { &mut self.nodes.get_unchecked_mut(i).val })
    }

    /// Give the hash part back (a pooled table's reset).
    pub(crate) fn drop_hash_part(&mut self) {
        self.nodes = Box::new([]);
        self.node_mask = u64::MAX;
    }
}
