//! The trace a side trace starts from, named without holding it.
//!
//! A recording is plain data: one with a tier move is cloned into its
//! trace's tier source, and the parent holds its side traces, so a
//! reference to the parent from the recording would make the two keep each
//! other alive. Traces are never removed from `Proto::traces`, nor entry
//! side traces from their root's list, so an index into each names the same
//! trace for as long as the prototype lives.

use super::*;

/// A root trace by its index in its head prototype's `traces`, or one of
/// the root's entry side traces by its place in the root's list.
#[derive(Clone, Copy, Debug)]
pub struct ParentId {
    /// The parent's head prototype.
    pub proto: Gc<Proto>,
    /// The parent's head pc, checked against the trace the indices name.
    pub head_pc: u32,
    /// The root's index in `proto`'s traces.
    pub root: u32,
    /// 0 for the root itself, `k` for its `k`th entry side trace.
    pub side: u8,
}

impl PartialEq for ParentId {
    fn eq(&self, o: &Self) -> bool {
        self.proto.ptr_eq(o.proto)
            && (self.head_pc, self.root, self.side) == (o.head_pc, o.root, o.side)
    }
}

impl ParentId {
    /// The trace this names; `None` when it names none.
    pub fn trace(&self) -> Option<TArc<CompiledTrace>> {
        let root = self
            .proto
            .traces
            .borrow()
            .get(self.root as usize)
            .cloned()?;
        let t = match self.side {
            0 => root,
            k => root
                .entry_sides
                .list
                .borrow()
                .get(k as usize - 1)
                .cloned()?,
        };
        debug_assert_eq!(t.head_pc, self.head_pc, "a parent id names another trace");
        Some(t)
    }

    /// The name of `t`, a root of `proto` or an entry side trace of one;
    /// `None` when it is neither.
    pub fn of(proto: Gc<Proto>, t: &CompiledTrace) -> Option<ParentId> {
        let traces = proto.traces.borrow();
        let id = |root: usize, side: usize| ParentId {
            proto,
            head_pc: t.head_pc,
            root: root as u32,
            side: side as u8,
        };
        if let Some(i) = traces.iter().position(|r| std::ptr::eq(&**r, t)) {
            return Some(id(i, 0));
        }
        traces.iter().enumerate().find_map(|(i, r)| {
            let sides = r.entry_sides.list.borrow();
            let k = sides.iter().position(|e| std::ptr::eq(&**e, t))?;
            Some(id(i, k + 1))
        })
    }
}
