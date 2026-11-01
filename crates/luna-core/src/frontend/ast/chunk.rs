//! The parsed chunk: the node arenas, the list vectors and the names.

use super::*;
use crate::runtime::mem::{LVec, MemOwner};

/// A parsed chunk: the top-level block plus the node arenas, the list
/// vectors the nodes' [`List`]s point into, and the chunk's names.
///
/// Walk it from [`Chunk::block`]: [`Chunk::stat`] and [`Chunk::expr`] give
/// the nodes, [`Chunk::list`] the ids and items of a [`List`], and
/// [`Chunk::name`] / [`Chunk::str`] the text of a [`Name`] or literal.
///
/// Its vectors come from an allocation context, which the chunk keeps
/// alive ([`Chunk::new`]); they are read through methods, so none can be
/// moved to a chunk on another context. The example compiles, the one
/// after it fails to.
///
/// ```
/// use luna_core::{frontend::parse, version::LuaVersion};
/// let a = parse(b"local x = 1", LuaVersion::Lua54).unwrap();
/// assert_eq!(a.stats().len(), a.stat_lines().len());
/// ```
///
/// ```compile_fail,E0616
/// use luna_core::{frontend::parse, version::LuaVersion};
/// let mut a = parse(b"local x = 1", LuaVersion::Lua54).unwrap();
/// let mut b = parse(b"local y = 2", LuaVersion::Lua54).unwrap();
/// std::mem::swap(&mut a.stat_lines, &mut b.stat_lines); // error[E0616]: field `stat_lines` is private
/// ```
#[derive(Debug)]
pub struct Chunk {
    pub(crate) exprs: LVec<Expr>,
    pub(crate) stats: LVec<Stat>,
    pub(crate) stat_lines: LVec<u32>,
    /// Top-level block (the script body).
    pub block: Block,
    /// line of the final `<eof>` token (PUC main-chunk `lastlinedefined`); the
    /// implicit final return is attributed here
    pub end_line: u32,
    pub(crate) names: Names,
    pub(crate) expr_lists: LVec<ExprId>,
    pub(crate) stat_lists: LVec<StatId>,
    pub(crate) name_lists: LVec<Name>,
    pub(crate) attrib_name_lists: LVec<AttribName>,
    pub(crate) field_lists: LVec<TableField>,
    pub(crate) arm_lists: LVec<IfArm>,
    pub(crate) fold_marks: LVec<(u32, u64)>,
    pub(crate) dot_keys: LVec<u32>,
    /// the allocation context the vectors come from
    mem: MemOwner,
}

/// One method per arena or list vector, and the names, read.
macro_rules! readers {
    ($($(#[$m:meta])* $f:ident: $t:ty;)*) => {
        impl Chunk {
            $(
                $(#[$m])*
                #[inline]
                pub fn $f(&self) -> &$t {
                    &self.$f
                }
            )*
        }
    };
}

readers! {
    /// Arena of all expression nodes; index with [`ExprId`].
    exprs: [Expr];
    /// Arena of all statement nodes; index with [`StatId`].
    stats: [Stat];
    /// starting source line of each statement, indexed by `StatId`
    stat_lines: [u32];
    /// The identifiers and string literals the nodes refer to.
    names: Names;
    /// Items of every `List<ExprId>`.
    expr_lists: [ExprId];
    /// Items of every `List<StatId>` (block bodies).
    stat_lists: [StatId];
    /// Items of every `List<Name>`.
    name_lists: [Name];
    /// Items of every `List<AttribName>`.
    attrib_name_lists: [AttribName];
    /// Items of every `List<TableField>`.
    field_lists: [TableField];
    /// Items of every `List<IfArm>`.
    arm_lists: [IfArm];
    /// For each `^` and `%`, in the order they were made: the id of the
    /// right operand and how many times `errno` had been set by then.
    fold_marks: [(u32, u64)];
    /// The ids of the keys written `obj.name` (string-literal keys made from
    /// a name), in increasing order.
    dot_keys: [u32];
}

impl Default for Block {
    fn default() -> Block {
        Block { stats: List::EMPTY }
    }
}

impl Chunk {
    /// An empty chunk whose vectors come from `mem`'s budget, through a
    /// context of the chunk's own when a clone of `mem` is kept elsewhere
    /// (by a Vm on another thread, say).
    pub fn new(mem: MemOwner) -> Chunk {
        Chunk::on(mem.unshared())
    }

    /// An empty chunk whose vectors come from `mem`'s context itself: the
    /// Vm's loads build their trees on its heap's context.
    pub(crate) fn on(mem: MemOwner) -> Chunk {
        let m = mem.mem();
        Chunk {
            exprs: LVec::new(m),
            stats: LVec::new(m),
            stat_lines: LVec::new(m),
            block: Block::default(),
            end_line: 0,
            names: Names::new(m),
            expr_lists: LVec::new(m),
            stat_lists: LVec::new(m),
            name_lists: LVec::new(m),
            attrib_name_lists: LVec::new(m),
            field_lists: LVec::new(m),
            arm_lists: LVec::new(m),
            fold_marks: LVec::new(m),
            dot_keys: LVec::new(m),
            mem,
        }
    }

    /// The allocation context the chunk's vectors come from.
    pub fn mem_owner(&self) -> &MemOwner {
        &self.mem
    }

    /// Borrow an expression node by id.
    pub fn expr(&self, id: ExprId) -> &Expr {
        &self.exprs[id.0 as usize]
    }

    /// Borrow a statement node by id.
    pub fn stat(&self, id: StatId) -> &Stat {
        &self.stats[id.0 as usize]
    }

    /// Starting source line of statement `id` (0 if unrecorded).
    pub fn stat_line(&self, id: StatId) -> u32 {
        self.stat_lines.get(id.0 as usize).copied().unwrap_or(0)
    }

    /// The items of a list.
    pub fn list<T: ListItem>(&self, l: List<T>) -> &[T] {
        &T::items(self)[l.range()]
    }

    /// The statements of a block, in source order.
    pub fn block_stats(&self, b: &Block) -> &[StatId] {
        self.list(b.stats)
    }

    /// The text of an identifier.
    pub fn name(&self, n: Name) -> &str {
        self.names.text(n.sym)
    }

    /// The bytes of a string literal (or of any entry of [`Chunk::names`]).
    pub fn str(&self, s: Sym) -> &[u8] {
        self.names.bytes(s)
    }

    /// How many times `errno` had been set when the operation with right
    /// operand `rhs` was made (see [`Chunk::fold_marks`]).
    pub fn fold_stamp(&self, rhs: ExprId) -> Option<u64> {
        let i = self.fold_marks.binary_search_by_key(&rhs.0, |m| m.0).ok()?;
        Some(self.fold_marks[i].1)
    }

    /// Whether `key` is the key of an `obj.name` (see [`Chunk::dot_keys`]).
    pub fn is_dot_key(&self, key: ExprId) -> bool {
        self.dot_keys.binary_search(&key.0).is_ok()
    }

    /// Store `items` as a new list.
    pub fn push_list<T: ListItem>(&mut self, items: &[T]) -> List<T> {
        let v = T::items_mut(self, list::Token(()));
        let start = v.len() as u32;
        v.extend_from_slice_or_abort(items);
        List::new(start, items.len() as u32)
    }
}
