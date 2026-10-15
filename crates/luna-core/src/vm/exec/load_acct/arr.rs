use crate::runtime::Heap;

/// One array PUC grows by doubling (`luaM_growvector`) and trims when the
/// function ends.
#[derive(Clone, Copy, Default)]
pub(super) struct Arr {
    pub(super) n: usize,
    pub(super) cap: usize,
}

impl Arr {
    /// One more element of `elem` bytes.
    pub(super) fn push(&mut self, heap: &mut Heap, elem: usize) {
        if self.n + 1 > self.cap {
            let new = if self.cap < 2 { 4 } else { self.cap * 2 };
            heap.acct_block(self.cap * elem, new * elem);
            self.cap = new;
        }
        self.n += 1;
    }

    /// Grown to hold `n` (the parser's descriptor arrays, which shrink and
    /// grow again).
    pub(super) fn reach(&mut self, heap: &mut Heap, n: usize, elem: usize) {
        while self.cap < n {
            let new = if self.cap < 2 { 4 } else { self.cap * 2 };
            heap.acct_block(self.cap * elem, new * elem);
            self.cap = new;
        }
        self.n = n;
    }

    /// Trimmed to its elements (`luaM_shrinkvector`).
    pub(super) fn trim(&mut self, heap: &mut Heap, elem: usize) {
        heap.acct_block(self.cap * elem, self.n * elem);
        self.cap = self.n;
    }
}
