//! The hash-part entry of a table, and the string-key probes over it.

use crate::runtime::heap::Gc;
use crate::runtime::mem::{BlockKind, MemRef, Oom};
use crate::runtime::value::Value;

/// A hash-part entry (PUC `Node`): the key is kept as its tag and its
/// payload at the offsets a `Value` has them, so the first 16 bytes read
/// as the key ([`Node::key`]), and `flags`, `khash` and `next` live in what
/// is a `Value`'s padding (PUC `NodeKey`). 32 bytes, so a chain step is a
/// shift.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct Node {
    pub(super) key_tag: u8,
    /// [`DEAD_KEY`] and [`NEG_ZERO`]
    flags: u8,
    /// a string key's hash, its low 16 bits: where the key goes in a hash
    /// part of up to 2^16 nodes, known without loading the string (which
    /// rehashing and collisions would load in no useful order)
    pub(super) khash: u16,
    /// absolute index of the next node in this chain, or NONE
    pub(super) next: i32,
    pub(super) key_payload: std::mem::MaybeUninit<u64>,
    pub(super) val: Value,
}

pub(super) const NONE: i32 = -1;

/// PUC `setdeadkey` analogue: the key was a collectable that got swept out
/// of a weak table; the key is now nil, and this flag tells `insert_new`
/// that the slot still sits in a chain (its `next` is kept) rather than
/// being free.
const DEAD_KEY: u8 = 1;
/// The key is integer 0 but was stored as -0 under 5.1/5.2, which keep a
/// new key as it was given; iteration hands it back that way.
const NEG_ZERO: u8 = 2;
/// With [`DEAD_KEY`]: the dead key was a string, and the payload keeps its
/// hash; without, the payload keeps the number an object key hashed by
/// (`pointer_hash`), so the dead node's main position stays known.
const DEAD_STR: u8 = 4;

/// The largest node mask [`Node::khash`] covers.
pub(super) const KHASH_MASK: u32 = u16::MAX as u32;

/// What [`Node::khash`] holds for `key`.
#[inline(always)]
pub(super) fn khash_of(key: Value) -> u16 {
    match key {
        Value::Str(s) => s.hash() as u16,
        _ => 0,
    }
}

impl Node {
    pub(super) const EMPTY: Node = Node {
        key_tag: crate::runtime::value::tag::NIL,
        flags: 0,
        khash: 0,
        next: NONE,
        key_payload: std::mem::MaybeUninit::uninit(),
        val: Value::Nil,
    };

    /// A live entry; `khash` is [`khash_of`] the key.
    #[inline(always)]
    pub(super) fn new(key: Value, val: Value, next: i32, khash: u16) -> Node {
        let mut n = Node {
            val,
            next,
            khash,
            ..Node::EMPTY
        };
        n.set_key(key);
        n
    }

    /// See [`DEAD_KEY`].
    #[inline(always)]
    pub(super) fn dead_key(&self) -> bool {
        self.flags & DEAD_KEY != 0
    }

    /// The key goes dead (PUC `setdeadkey`): `hash` is what it hashed by,
    /// a string's hash (`str`) or an object's number, kept in the payload
    /// for its main position.
    pub(super) fn set_dead_key(&mut self, hash: u32, str: bool) {
        self.key_tag = crate::runtime::value::tag::NIL;
        self.flags |= DEAD_KEY | if str { DEAD_STR } else { 0 };
        self.key_payload = std::mem::MaybeUninit::new(u64::from(hash));
    }

    /// A dead key's hash and whether it was a string's.
    #[inline]
    pub(super) fn dead_hash(&self) -> (u32, bool) {
        // SAFETY: `set_dead_key` wrote the payload as a `u64`
        let h = unsafe { self.key_payload.assume_init() } as u32;
        (h, self.flags & DEAD_STR != 0)
    }

    /// See [`NEG_ZERO`].
    #[inline]
    pub(super) fn neg_zero(&self) -> bool {
        self.flags & NEG_ZERO != 0
    }

    pub(super) fn set_neg_zero(&mut self) {
        self.flags |= NEG_ZERO;
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
        if self.neg_zero() {
            Value::Float(-0.0)
        } else {
            self.key()
        }
    }

    /// Store `key`, leaving the flags, `khash` and `next` as they are.
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
            // SAFETY: a string key's payload starts with its pointer (on a
            // 32-bit target the word's other half is padding)
            && unsafe { *(self.key_payload.as_ptr() as *const usize) } == key.as_ptr() as usize
    }

    /// True when the slot holds no key and sits in no chain.
    #[inline(always)]
    pub(super) fn is_free(&self) -> bool {
        self.key_tag == crate::runtime::value::tag::NIL && !self.dead_key()
    }
}

impl super::Table {
    /// The node holding the string key `key`, found by pointer (PUC
    /// `luaH_getshortstr`). Exact for a short (interned) string; for a long
    /// one a hit is exact and a miss proves nothing.
    #[inline(always)]
    fn str_node_by_ptr(&self, key: Gc<crate::runtime::string::LuaStr>) -> Option<usize> {
        self.hook_find_node(Value::Str(key));
        let mask = self.node_mask;
        if mask >> 31 != 0 {
            return None;
        }
        // a short string's hash is set when it is interned; a long one's
        // may still be the seed, which only makes a hit unlikely
        let mut idx = (key.stored_hash() & mask) as usize;
        loop {
            // SAFETY: the main position is masked to the node count and
            // every `next` link is a node index written by `insert_new`
            let node = unsafe { &*self.nodes.add(idx) };
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
        Some(unsafe { &(*self.nodes.add(i)).val })
    }

    /// [`Self::str_slot_by_ptr`] for a write.
    #[inline(always)]
    pub(crate) fn str_slot_by_ptr_mut(
        &mut self,
        key: Gc<crate::runtime::string::LuaStr>,
    ) -> Option<&mut Value> {
        let i = self.str_node_by_ptr(key)?;
        // SAFETY: a node index found above
        Some(unsafe { &mut (*self.nodes.add(i)).val })
    }

    /// The hash part.
    #[inline(always)]
    pub(crate) fn nodes(&self) -> &[Node] {
        // SAFETY: `nodes` holds `node_mask + 1` nodes (0 when the mask is
        // `u32::MAX`; the pointer is then dangling, which a zero-length
        // slice allows)
        unsafe { std::slice::from_raw_parts(self.nodes, self.node_mask.wrapping_add(1) as usize) }
    }

    /// The hash part, for a write.
    #[inline(always)]
    pub(crate) fn nodes_mut(&mut self) -> &mut [Node] {
        // SAFETY: as in `nodes`; `&mut self` makes the access exclusive
        unsafe {
            std::slice::from_raw_parts_mut(self.nodes, self.node_mask.wrapping_add(1) as usize)
        }
    }

    /// Install `nodes`, a block of `len` nodes (0 or a power of two) from
    /// [`Table::alloc_nodes`], as the hash part. The previous one must
    /// have been taken already.
    pub(super) fn set_hash_part(&mut self, nodes: *mut Node, len: usize) {
        debug_assert!(len.is_power_of_two() || len == 0);
        self.node_mask = (len as u32).wrapping_sub(1);
        self.nodes = nodes;
    }

    /// Take the hash part out, leaving an empty one: its block and length,
    /// for [`Table::free_nodes`].
    /// The number of nodes in the hash part (0 for none).
    pub(crate) fn node_count(&self) -> usize {
        self.node_mask.wrapping_add(1) as usize
    }

    pub(super) fn take_hash_part(&mut self) -> (*mut Node, usize) {
        let len = self.node_mask.wrapping_add(1) as usize;
        let p = self.nodes;
        self.nodes = std::ptr::NonNull::dangling().as_ptr();
        self.node_mask = u32::MAX;
        (p, len)
    }

    /// A block of `len` empty nodes from `mem` (dangling for 0).
    pub(super) fn alloc_nodes(mem: MemRef, len: usize) -> Result<*mut Node, Oom> {
        if len == 0 {
            return Ok(std::ptr::NonNull::dangling().as_ptr());
        }
        let layout = std::alloc::Layout::array::<Node>(len).expect("hash part within MAX_ASIZE");
        let p = match mem.ctx().alloc(layout, BlockKind::Other) {
            Some(p) => p.cast::<Node>().as_ptr(),
            None => return Err(Oom(mem)),
        };
        for i in 0..len {
            // SAFETY: the block holds `len` nodes
            unsafe { p.add(i).write(Node::EMPTY) };
        }
        Ok(p)
    }

    /// Give back a block of `len` nodes from [`Table::alloc_nodes`].
    ///
    /// # Safety
    /// `p` and `len` came from `alloc_nodes(mem, len)` (or
    /// `take_hash_part` of a part made by it) and `p` is not used again.
    pub(super) unsafe fn free_nodes(mem: MemRef, p: *mut Node, len: usize) {
        if len != 0 {
            let layout = std::alloc::Layout::array::<Node>(len).expect("an allocated part");
            // SAFETY: the caller's contract
            unsafe {
                mem.ctx()
                    .free(std::ptr::NonNull::new_unchecked(p.cast()), layout)
            };
        }
    }

    /// Give the hash part back (freeing a table, or a pooled table's
    /// reset).
    pub(crate) fn drop_hash_part(&mut self, mem: MemRef) {
        let (p, len) = self.take_hash_part();
        // SAFETY: the part came from `alloc_nodes(mem, len)` and was just
        // taken out of the table
        unsafe { Self::free_nodes(mem, p, len) };
    }
}
