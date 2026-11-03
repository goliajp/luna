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
/// Only the tree's own item types are list items, and only the chunk adds
/// to its vectors ([`Chunk::push_list`]): a vector moved to another chunk
/// would outlive the context it was allocated from. Each example compiles
/// and the one after it fails to: the vector is out of reach by its type
/// and through a bound alike.
///
/// ```
/// use luna_core::{frontend::{parse, ast::{ExprId, ListItem}}, version::LuaVersion};
/// let a = parse(b"local x = f(1, 2)", LuaVersion::Lua54).unwrap();
/// let _ = ExprId::items(&a);
/// ```
///
/// ```compile_fail,E0599
/// use luna_core::{frontend::{parse, ast::{ExprId, ListItem}}, version::LuaVersion};
/// let mut a = parse(b"local x = f(1, 2)", LuaVersion::Lua54).unwrap();
/// let mut b = parse(b"local y = g(3, 4)", LuaVersion::Lua54).unwrap();
/// std::mem::swap(ExprId::items_mut(&mut a), ExprId::items_mut(&mut b)); // error[E0599]
/// ```
///
/// ```
/// use luna_core::frontend::ast::{Chunk, ListItem};
/// fn count<T: ListItem>(c: &Chunk) -> usize {
///     T::items(c).len()
/// }
/// ```
///
/// ```compile_fail,E0603
/// use luna_core::frontend::ast::{Chunk, ListItem};
/// fn swap<T: ListItem>(a: &mut Chunk, b: &mut Chunk) {
///     let t = || luna_core::frontend::ast::list::Token(()); // error[E0603]
///     std::mem::swap(T::items_mut(a, t()), T::items_mut(b, t()));
/// }
/// ```
///
/// ```compile_fail,E0277
/// use luna_core::frontend::ast::{Chunk, ListItem};
/// fn swap<T: ListItem>(a: &mut Chunk, b: &mut Chunk) {
///     // the token proving the caller is in the crate cannot be made here
///     let (x, y) = (Default::default(), Default::default());
///     std::mem::swap(T::items_mut(a, x), T::items_mut(b, y)); // error[E0277]
/// }
/// ```
pub trait ListItem: Copy + Sized + store::ListStore {
    /// The chunk's items of this type.
    fn items(chunk: &Chunk) -> &[Self];
}

pub(super) use store::Token;

mod store {
    use super::*;

    /// Where a list item type's vector is in the chunk. Outside the crate
    /// it cannot be implemented, and a bound `T: ListItem` finds
    /// `items_mut` but cannot call it: only the crate makes a [`Token`].
    pub trait ListStore: Sized {
        fn items_mut(chunk: &mut Chunk, _: Token) -> &mut LVec<Self>;
    }

    /// The proof a caller of [`ListStore::items_mut`] is in the crate.
    pub struct Token(pub(in crate::frontend::ast) ());
}

macro_rules! list_item {
    ($t:ty, $field:ident) => {
        impl ListItem for $t {
            fn items(chunk: &Chunk) -> &[Self] {
                &chunk.$field
            }
        }
        impl store::ListStore for $t {
            fn items_mut(chunk: &mut Chunk, _: store::Token) -> &mut LVec<Self> {
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
