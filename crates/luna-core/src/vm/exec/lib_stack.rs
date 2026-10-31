//! What a library function keeps where PUC's keeps it on its stack: roots
//! for the collector, and the order PUC pushes and pops them in.

use super::*;

/// The values library functions hold as PUC's hold them on the stack.
/// What a function pushes is let go when the [`Vm::lib_scope`] around it
/// ends, by an error too, as an error takes PUC's C stack down to the
/// protected call; only a `luaL_Buffer` takes its own strings off sooner.
#[derive(Default)]
pub(crate) struct LibStack {
    vals: Vec<Value>,
    /// debug builds: the scopes open, which a buffer is made inside
    #[cfg(debug_assertions)]
    scopes: u32,
}

impl LibStack {
    /// Debug builds: whether some [`Vm::lib_scope`] is open.
    #[cfg(debug_assertions)]
    pub(crate) fn in_scope(&self) -> bool {
        self.scopes > 0
    }

    /// Back to height `at` (`lua_settop`): what was pushed above it is let
    /// go.
    #[inline]
    pub(crate) fn pop_to(&mut self, at: usize) {
        debug_assert!(at <= self.vals.len(), "a height above the stack");
        self.vals.truncate(at);
    }

    #[inline]
    pub(crate) fn push(&mut self, v: Value) {
        self.vals.push(v);
    }

    #[inline]
    pub(crate) fn pop(&mut self) -> Option<Value> {
        self.vals.pop()
    }

    #[inline]
    pub(crate) fn extend(&mut self, vs: impl IntoIterator<Item = Value>) {
        self.vals.extend(vs);
    }

    #[inline]
    pub(crate) fn extend_from_slice(&mut self, vs: &[Value]) {
        self.vals.extend_from_slice(vs);
    }

    /// `lua_pop(L, n)`.
    #[inline]
    pub(crate) fn pop_n(&mut self, n: usize) {
        self.vals.truncate(self.vals.len() - n);
    }

    /// `lua_remove` of the value at `i`.
    #[inline]
    pub(crate) fn remove(&mut self, i: usize) -> Value {
        self.vals.remove(i)
    }

    /// `join` the top `total` values into one, on the values themselves
    /// (`lua_concat` works on a stack of its own): it sees only them and
    /// brings the count of those left down from `total`, to 1 when it
    /// succeeds; the stack keeps that many.
    #[inline]
    pub(crate) fn join_top<E>(
        &mut self,
        total: usize,
        join: impl FnOnce(&mut [Value], &mut usize) -> Result<(), E>,
    ) -> Result<(), E> {
        let below = self.vals.len() - total;
        let mut left = total;
        let r = join(&mut self.vals[below..], &mut left);
        self.vals.truncate(below + left);
        r
    }
}

impl std::ops::Deref for LibStack {
    type Target = [Value];
    #[inline]
    fn deref(&self) -> &[Value] {
        &self.vals
    }
}

impl<'a> IntoIterator for &'a LibStack {
    type Item = &'a Value;
    type IntoIter = std::slice::Iter<'a, Value>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.vals.iter()
    }
}

impl std::ops::DerefMut for LibStack {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Value] {
        &mut self.vals
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
        self.lib_scope_at(|vm, _| f(vm))
    }

    /// [`Vm::lib_scope`], `f` told where what it pushes starts.
    #[inline]
    pub(crate) fn lib_scope_at<R>(&mut self, f: impl FnOnce(&mut Vm, usize) -> R) -> R {
        let at = self.lib_roots.len();
        #[cfg(debug_assertions)]
        {
            self.lib_roots.scopes += 1;
        }
        let r = f(self, at);
        self.lib_roots.pop_to(at);
        #[cfg(debug_assertions)]
        {
            self.lib_roots.scopes -= 1;
        }
        r
    }
}
