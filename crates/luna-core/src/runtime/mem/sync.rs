//! The state contexts share, which with the `send` feature Vms on other
//! threads may reach at once: atomics and locks there, cells otherwise.

/// A count of bytes, owners or users.
pub(super) struct Count {
    #[cfg(not(feature = "send"))]
    n: std::cell::Cell<usize>,
    #[cfg(feature = "send")]
    n: std::sync::atomic::AtomicUsize,
}

#[cfg(not(feature = "send"))]
impl Count {
    pub(super) const fn new(n: usize) -> Count {
        Count {
            n: std::cell::Cell::new(n),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self) -> usize {
        self.n.get()
    }

    #[inline(always)]
    pub(super) fn add(&self, d: usize) {
        self.n.set(self.n.get() + d);
    }

    /// Whether the count is 1.
    pub(super) fn is_one(&self) -> bool {
        self.n.get() == 1
    }

    /// Takes `d` off; the count left.
    #[inline(always)]
    pub(super) fn sub(&self, d: usize) -> usize {
        let n = self.n.get() - d;
        self.n.set(n);
        n
    }
}

#[cfg(feature = "send")]
impl Count {
    pub(super) const fn new(n: usize) -> Count {
        Count {
            n: std::sync::atomic::AtomicUsize::new(n),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self) -> usize {
        self.n.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[inline(always)]
    pub(super) fn add(&self, d: usize) {
        self.n.fetch_add(d, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the count is 1: an owner seeing it is the last, and sees
    /// what the owners dropped before did (as `sub`).
    pub(super) fn is_one(&self) -> bool {
        self.n.load(std::sync::atomic::Ordering::Acquire) == 1
    }

    /// Takes `d` off; the count left. The last of an owner count to go
    /// sees every earlier owner's writes (as `Arc`'s drop).
    #[inline(always)]
    pub(super) fn sub(&self, d: usize) -> usize {
        self.n.fetch_sub(d, std::sync::atomic::Ordering::AcqRel) - d
    }
}

/// A value one user at a time changes.
pub(super) struct Lock<T> {
    #[cfg(not(feature = "send"))]
    v: std::cell::RefCell<T>,
    #[cfg(feature = "send")]
    v: std::sync::Mutex<T>,
}

impl<T> Lock<T> {
    pub(super) const fn new(v: T) -> Lock<T> {
        Lock {
            #[cfg(not(feature = "send"))]
            v: std::cell::RefCell::new(v),
            #[cfg(feature = "send")]
            v: std::sync::Mutex::new(v),
        }
    }

    pub(super) fn lock(&self) -> impl std::ops::DerefMut<Target = T> + '_ {
        #[cfg(not(feature = "send"))]
        {
            self.v.borrow_mut()
        }
        // a policy that panicked leaves the lock poisoned; the budget goes
        // on as a `RefCell` does once the panic has unwound
        #[cfg(feature = "send")]
        {
            self.v
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }
}

/// A value read and replaced whole. With the `send` feature a read takes
/// no lock: the value sits in a block of its own, swapped in whole, and the
/// blocks it replaced stay until the slot goes, as a reader on another
/// thread may still be reading one.
pub(super) struct Slot<T: Copy> {
    #[cfg(not(feature = "send"))]
    v: std::cell::Cell<T>,
    #[cfg(feature = "send")]
    v: std::sync::atomic::AtomicPtr<T>,
    /// the blocks `v` held before, by address: a `Box` made of one while
    /// a reader may still hold its address would claim it alone
    #[cfg(feature = "send")]
    replaced: std::sync::Mutex<Vec<std::sync::atomic::AtomicPtr<T>>>,
}

#[cfg(not(feature = "send"))]
impl<T: Copy> Slot<T> {
    pub(super) fn new(v: T) -> Slot<T> {
        Slot {
            v: std::cell::Cell::new(v),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self) -> T {
        self.v.get()
    }

    pub(super) fn set(&self, v: T) {
        self.v.set(v);
    }
}

#[cfg(feature = "send")]
impl<T: Copy> Slot<T> {
    pub(super) fn new(v: T) -> Slot<T> {
        Slot {
            v: std::sync::atomic::AtomicPtr::new(Box::into_raw(Box::new(v))),
            replaced: std::sync::Mutex::new(Vec::new()),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self) -> T {
        let p = self.v.load(std::sync::atomic::Ordering::Acquire);
        // SAFETY: `p` came from `Box::into_raw` and is freed only with the
        // slot, the blocks replaced included
        unsafe { *p }
    }

    pub(super) fn set(&self, v: T) {
        let p = Box::into_raw(Box::new(v));
        let old = self.v.swap(p, std::sync::atomic::Ordering::AcqRel);
        self.replaced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(std::sync::atomic::AtomicPtr::new(old));
    }
}

#[cfg(feature = "send")]
impl<T: Copy> Drop for Slot<T> {
    fn drop(&mut self) {
        let replaced = self
            .replaced
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for p in std::iter::once(&mut self.v).chain(replaced.iter_mut()) {
            // SAFETY: each pointer came from `Box::into_raw` once, and the
            // slot is going: nothing reads it any more
            drop(unsafe { Box::from_raw(*p.get_mut()) });
        }
    }
}
