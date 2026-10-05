//! The hash-part entry of a table, and the string-key probes over it.

use crate::runtime::heap::Gc;
use crate::runtime::mem::{BlockKind, MemRef};
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

impl super::Table {
    /// The node holding the string key `key`, found by pointer (PUC
    /// `luaH_getshortstr`). Exact for a short (interned) string; for a long
    /// one a hit is exact and a miss proves nothing.
    #[inline(always)]
    fn str_node_by_ptr(&self, key: Gc<crate::runtime::string::LuaStr>) -> Option<usize> {
        #[cfg(feature = "gc-verify")]
        self.verify_find_node_keys(Value::Str(key));
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
    #[cfg_attr(feature = "gc-verify", allow(dead_code))]
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
    pub(super) fn take_hash_part(&mut self) -> (*mut Node, usize) {
        let len = self.node_mask.wrapping_add(1) as usize;
        let p = self.nodes;
        self.nodes = std::ptr::NonNull::dangling().as_ptr();
        self.node_mask = u32::MAX;
        (p, len)
    }

    /// A block of `len` empty nodes from `mem` (dangling for 0).
    pub(super) fn alloc_nodes(mem: MemRef, len: usize) -> *mut Node {
        if len == 0 {
            return std::ptr::NonNull::dangling().as_ptr();
        }
        let layout = std::alloc::Layout::array::<Node>(len).expect("hash part within MAX_ASIZE");
        let p = match mem.ctx().alloc(layout, BlockKind::Other) {
            Some(p) => p.cast::<Node>().as_ptr(),
            None => crate::runtime::mem::oom_abort(layout),
        };
        for i in 0..len {
            // SAFETY: the block holds `len` nodes
            unsafe { p.add(i).write(Node::EMPTY) };
        }
        p
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
