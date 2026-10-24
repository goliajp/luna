//! What a library function keeps where PUC's keeps it on its stack: roots
//! for the collector, and the order PUC pushes and pops them in.

use super::*;

/// The values library functions hold as PUC's hold them on the stack.
/// Taking it back to an earlier height goes through a [`LibMark`], so a
/// height read once is released once.
#[derive(Default)]
pub(crate) struct LibStack(Vec<Value>);

/// The height of the [`LibStack`] at some point, to go back to with
/// [`LibStack::release`].
#[must_use = "what was pushed since stays rooted until the mark is released"]
pub(crate) struct LibMark(usize);

impl LibMark {
    /// Where the values pushed since the mark start.
    #[inline]
    pub(crate) fn at(&self) -> usize {
        self.0
    }
}

impl LibStack {
    #[inline]
    pub(crate) fn mark(&self) -> LibMark {
        LibMark(self.0.len())
    }

    /// Back to the height of `m`: what was pushed since is let go.
    #[inline]
    pub(crate) fn release(&mut self, m: LibMark) {
        debug_assert!(m.0 <= self.0.len(), "a mark above the stack");
        self.0.truncate(m.0);
    }

    #[inline]
    pub(crate) fn push(&mut self, v: Value) {
        self.0.push(v);
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Option<Value> {
        self.0.pop()
    }

    #[inline]
    pub(crate) fn extend(&mut self, vs: impl IntoIterator<Item = Value>) {
        self.0.extend(vs);
    }

    #[inline]
    pub(crate) fn extend_from_slice(&mut self, vs: &[Value]) {
        self.0.extend_from_slice(vs);
    }

    /// `lua_pop(L, n)`.
    #[inline]
    pub(crate) fn pop_n(&mut self, n: usize) {
        self.0.truncate(self.0.len() - n);
    }

    /// `lua_remove` of the value at `i`.
    #[inline]
    pub(crate) fn remove(&mut self, i: usize) -> Value {
        self.0.remove(i)
    }

    /// The values themselves, for an operation that works on a stack of
    /// its own (`lua_concat`'s pieces).
    #[inline]
    pub(crate) fn values_mut(&mut self) -> &mut Vec<Value> {
        &mut self.0
    }
}

impl std::ops::Deref for LibStack {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        &self.0
    }
}

impl<'a> IntoIterator for &'a LibStack {
    type Item = &'a Value;
    type IntoIter = std::slice::Iter<'a, Value>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl std::ops::DerefMut for LibStack {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        &mut self.0
    }
}

impl Vm {
    /// Run `f` with `vals` held as roots, let go after.
    #[inline]
    pub(crate) fn rooted<R>(&mut self, vals: &[Value], f: impl FnOnce(&mut Vm) -> R) -> R {
        self.lib_scope(|vm| {
            vm.lib_roots.extend_from_slice(vals);
            f(vm)
        })
    }

    /// Runs `f`; what it leaves on the stack is let go when it returns,
    /// by an error too, as an error takes PUC's C stack down to the
    /// protected call.
    #[inline]
    pub(crate) fn lib_scope<R>(&mut self, f: impl FnOnce(&mut Vm) -> R) -> R {
        let m = self.lib_roots.mark();
        let r = f(self);
        self.lib_roots.release(m);
        r
    }
}
