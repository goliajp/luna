//! The register tags of a trace's typed side exits, held in one block.

use super::ExitTag;

/// The register tags each typed side exit of a trace restores, all in one
/// block: exit `i` resumes at [`cont_pc(i)`](Self::cont_pc) and restores
/// through [`tags(i)`](Self::tags).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExitTagTable {
    /// `(cont_pc, start, end)` per exit: its tags are `tags[start..end]`
    /// (both bounds kept, so the dispatcher reads them in one go).
    index: Box<[(u32, u32, u32)]>,
    tags: Box<[ExitTag]>,
}

impl ExitTagTable {
    /// The table of `exits`, each `(cont_pc, tags)`, in order.
    ///
    /// # Panics
    /// If the tags of all exits together number more than `u32::MAX`.
    pub fn from_exits<'a>(exits: impl IntoIterator<Item = (u32, &'a [ExitTag])>) -> Self {
        let mut index = Vec::new();
        let mut tags = Vec::new();
        for (pc, t) in exits {
            let start = tags.len() as u32;
            tags.extend_from_slice(t);
            let end = u32::try_from(tags.len()).expect("exit tags past u32::MAX");
            index.push((pc, start, end));
        }
        ExitTagTable {
            index: index.into(),
            tags: tags.into(),
        }
    }

    /// How many exits the table holds.
    #[inline]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether the table holds no exit.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The pc exit `i` resumes at.
    ///
    /// # Panics
    /// If `i >= self.len()`.
    #[inline]
    pub fn cont_pc(&self, i: usize) -> u32 {
        self.index[i].0
    }

    /// The tags exit `i` restores through.
    ///
    /// # Panics
    /// If `i >= self.len()`.
    #[inline]
    pub fn tags(&self, i: usize) -> &[ExitTag] {
        let (_, start, end) = self.index[i];
        &self.tags[start as usize..end as usize]
    }

    /// Exit `i`'s `(cont_pc, tags)`, or `None` past the end.
    #[inline]
    pub fn get(&self, i: usize) -> Option<(u32, &[ExitTag])> {
        (i < self.len()).then(|| (self.cont_pc(i), self.tags(i)))
    }

    /// Each exit's `(cont_pc, tags)`, in order.
    pub fn iter(&self) -> ExitTagIter<'_> {
        self.into_iter()
    }
}

impl<'a> IntoIterator for &'a ExitTagTable {
    type Item = (u32, &'a [ExitTag]);
    type IntoIter = ExitTagIter<'a>;

    fn into_iter(self) -> ExitTagIter<'a> {
        ExitTagIter { table: self, i: 0 }
    }
}

/// The exits of an [`ExitTagTable`], in order.
#[derive(Clone, Debug)]
pub struct ExitTagIter<'a> {
    table: &'a ExitTagTable,
    i: usize,
}

impl<'a> Iterator for ExitTagIter<'a> {
    type Item = (u32, &'a [ExitTag]);

    fn next(&mut self) -> Option<Self::Item> {
        let e = self.table.get(self.i)?;
        self.i += 1;
        Some(e)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.table.len() - self.i;
        (n, Some(n))
    }
}

impl ExactSizeIterator for ExitTagIter<'_> {}

// past the last exit `next` stays at `None` without moving
impl std::iter::FusedIterator for ExitTagIter<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_keep_their_own_tags() {
        let a = [ExitTag::Int, ExitTag::Untouched];
        let b: [ExitTag; 0] = [];
        let c = [ExitTag::Str, ExitTag::Nil, ExitTag::Float];
        let t = ExitTagTable::from_exits([(3, &a[..]), (7, &b[..]), (11, &c[..])]);
        assert_eq!(t.len(), 3);
        assert_eq!((t.cont_pc(0), t.tags(0)), (3, &a[..]));
        assert_eq!((t.cont_pc(1), t.tags(1)), (7, &b[..]));
        assert_eq!((t.cont_pc(2), t.tags(2)), (11, &c[..]));
        assert_eq!(t.get(3), None);
        let all: Vec<_> = (&t).into_iter().collect();
        assert_eq!(all, t.iter().collect::<Vec<_>>());
        assert_eq!(all.len(), 3);
        assert!(ExitTagTable::default().is_empty());
    }
}
