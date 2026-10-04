//! Lists of a tree: ranges into vectors of the chunk.

use super::*;
use crate::runtime::mem::LVec;
use std::marker::PhantomData;

/// A list of `T` stored in a [`Chunk`]: `len` items from `start` in the
/// chunk's vector for `T` ([`ListItem`]). Read it with [`Chunk::list`].
pub struct List<T> {
    /// Offset of the first item.
    pub start: u32,
    /// Number of items.
    pub len: u32,
    _item: PhantomData<fn() -> T>,
}

impl<T> List<T> {
    /// The empty list.
    pub const EMPTY: List<T> = List::new(0, 0);

    /// The `len` items from `start`.
    pub const fn new(start: u32, len: u32) -> List<T> {
        List {
            start,
            len,
            _item: PhantomData,
        }
    }

    /// Whether the list has no items.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

impl<T> Clone for List<T> {
    fn clone(&self) -> List<T> {
        *self
    }
}

impl<T> Copy for List<T> {}

impl<T> PartialEq for List<T> {
    fn eq(&self, o: &List<T>) -> bool {
        self.start == o.start && self.len == o.len
    }
}

impl<T> Eq for List<T> {}

impl<T> std::fmt::Debug for List<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "List({}..+{})", self.start, self.len)
    }
}

/// An item type a [`List`] can hold, with the [`Chunk`] vector it lives in.
pub trait ListItem: Copy + Sized {
    /// The chunk's vector of this item type.
    fn items(chunk: &Chunk) -> &LVec<Self>;
    /// The same vector, to add to.
    fn items_mut(chunk: &mut Chunk) -> &mut LVec<Self>;
}

macro_rules! list_item {
    ($t:ty, $field:ident) => {
        impl ListItem for $t {
            fn items(chunk: &Chunk) -> &LVec<Self> {
                &chunk.$field
            }
            fn items_mut(chunk: &mut Chunk) -> &mut LVec<Self> {
                &mut chunk.$field
            }
        }
    };
}

list_item!(ExprId, expr_lists);
list_item!(StatId, stat_lists);
list_item!(Name, name_lists);
list_item!(AttribName, attrib_name_lists);
list_item!(TableField, field_lists);
list_item!(IfArm, arm_lists);
