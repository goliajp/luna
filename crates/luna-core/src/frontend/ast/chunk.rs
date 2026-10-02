//! The parsed chunk: the node arenas, the list vectors and the names.

use super::*;

/// A parsed chunk: the top-level block plus the node arenas, the list
/// vectors the nodes' [`List`]s point into, and the chunk's names.
///
/// Walk it from [`Chunk::block`]: [`Chunk::stat`] and [`Chunk::expr`] give
/// the nodes, [`Chunk::list`] the ids and items of a [`List`], and
/// [`Chunk::name`] / [`Chunk::str`] the text of a [`Name`] or literal.
#[derive(Clone, Debug, Default)]
pub struct Chunk {
    /// Arena of all expression nodes; index with [`ExprId`].
    pub exprs: Vec<Expr>,
    /// Arena of all statement nodes; index with [`StatId`].
    pub stats: Vec<Stat>,
    /// starting source line of each statement, indexed by `StatId`
    pub stat_lines: Vec<u32>,
    /// Top-level block (the script body).
    pub block: Block,
    /// line of the final `<eof>` token (PUC main-chunk `lastlinedefined`); the
    /// implicit final return is attributed here
    pub end_line: u32,
    /// The identifiers and string literals the nodes refer to.
    pub names: Names,
    /// Items of every `List<ExprId>`.
    pub expr_lists: Vec<ExprId>,
    /// Items of every `List<StatId>` (block bodies).
    pub stat_lists: Vec<StatId>,
    /// Items of every `List<Name>`.
    pub name_lists: Vec<Name>,
    /// Items of every `List<AttribName>`.
    pub attrib_name_lists: Vec<AttribName>,
    /// Items of every `List<TableField>`.
    pub field_lists: Vec<TableField>,
    /// Items of every `List<IfArm>`.
    pub arm_lists: Vec<IfArm>,
}

impl Default for Block {
    fn default() -> Block {
        Block { stats: List::EMPTY }
    }
}

impl Chunk {
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

    /// Store `items` as a new list.
    pub fn push_list<T: ListItem>(&mut self, items: &[T]) -> List<T> {
        let v = T::items_mut(self);
        let start = v.len() as u32;
        v.extend_from_slice(items);
        List::new(start, items.len() as u32)
    }
}
