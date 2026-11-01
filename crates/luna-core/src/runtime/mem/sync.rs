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

    /// Takes `d` off; the count left. The last of an owner count to go
    /// sees every earlier owner's writes (as `Arc`'s drop).
    #[inline(always)]
    pub(super) fn sub(&self, d: usize) -> usize {
        self.n.fetch_sub(d, std::sync::atomic::Ordering::AcqRel) - d
    }
}

/// A flag set once, by the first to ask.
pub(super) struct Once {
    #[cfg(not(feature = "send"))]
    set: std::cell::Cell<bool>,
    #[cfg(feature = "send")]
    set: std::sync::atomic::AtomicBool,
}

impl Once {
    pub(super) const fn new() -> Once {
        Once {
            #[cfg(not(feature = "send"))]
            set: std::cell::Cell::new(false),
            #[cfg(feature = "send")]
            set: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Sets the flag: whether this call is the one that did.
    pub(super) fn take(&self) -> bool {
        #[cfg(not(feature = "send"))]
        {
            !self.set.replace(true)
        }
        #[cfg(feature = "send")]
        {
            !self.set.swap(true, std::sync::atomic::Ordering::AcqRel)
        }
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
        #[cfg(feature = "send")]
        {
            self.v.lock().unwrap()
        }
    }
}

/// A value read and replaced whole.
pub(super) struct Slot<T: Copy> {
    #[cfg(not(feature = "send"))]
    v: std::cell::Cell<T>,
    #[cfg(feature = "send")]
    v: std::sync::Mutex<T>,
}

impl<T: Copy> Slot<T> {
    pub(super) const fn new(v: T) -> Slot<T> {
        Slot {
            #[cfg(not(feature = "send"))]
            v: std::cell::Cell::new(v),
            #[cfg(feature = "send")]
            v: std::sync::Mutex::new(v),
        }
    }

    #[inline(always)]
    pub(super) fn get(&self) -> T {
        #[cfg(not(feature = "send"))]
        {
            self.v.get()
        }
        #[cfg(feature = "send")]
        {
            *self.v.lock().unwrap()
        }
    }

    pub(super) fn set(&self, v: T) {
        #[cfg(not(feature = "send"))]
        {
            self.v.set(v);
        }
        #[cfg(feature = "send")]
        {
            *self.v.lock().unwrap() = v;
        }
    }
}
