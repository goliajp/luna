//! Marking an object with what it holds as soon as it is marked, or
//! later: the objects PUC traverses at once, and luna's own objects,
//! which PUC does not have, marked after the counted marking without
//! taking part in its order or its work.

use super::*;

impl Marker {
    /// 5.2 and 5.3 count a string's or a userdata's size as the work of
    /// marking it.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[cold]
    #[inline(never)]
    pub(super) unsafe fn count_leaf(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            self.leaf_bytes += match (*h).tag {
                // a string of luna's own is no string of PUC's
                ObjTag::Str if (*h).sub & UNCOUNTED != 0 => 0,
                ObjTag::Str => (self.leaf_hdr + (*(h as *const LuaStr)).len() + 1) as i64,
                // one standing for PUC's has PUC's size
                ObjTag::Userdata => (*(h as *const Userdata)).puc_size as i64,
                // 5.2: an open one stays gray, counting nothing
                ObjTag::Upvalue => match (*(h as *const crate::runtime::function::Upvalue)).state {
                    crate::runtime::function::UpvalState::Closed(_) => self.leaf_upval as i64,
                    _ => 0,
                },
                _ => 0,
            };
        }
    }

    /// Whether PUC marks `h` with what it holds as soon as it marks it.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline(always)]
    pub(super) unsafe fn at_once(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract
        match unsafe { (*h).tag } {
            ObjTag::Upvalue => true,
            // up to 5.3 every userdata, from 5.4 one without user values,
            // which luna's are
            ObjTag::Userdata => true,
            _ => false,
        }
    }

    /// What a userdata `h` marked at once holds: its metatable, then its
    /// user value, a chain of userdata followed in a loop as PUC 5.3's
    /// `reallymarkobject` does, so that it takes no stack.
    ///
    /// # Safety
    /// `h` heads an allocated userdata, black already.
    pub(super) unsafe fn userdata_at_once(&mut self, mut h: *mut GcHeader) {
        // SAFETY: the caller's contract; each userdata followed is held by
        // the one before, allocated while marking
        unsafe {
            loop {
                let u = &*(h as *const Userdata);
                if let Some(mt) = u.metatable() {
                    self.mark(mt);
                }
                self.mark_env51((*h).cls);
                if let UserdataPayload::Host {
                    data,
                    trace_fn: Some(trace_fn),
                    ..
                } = &u.payload
                {
                    let mut um = crate::vm::UserdataMarker::__new_internal(self);
                    trace_fn(data, &mut um);
                }
                let Value::Userdata(next) = u.user_value else {
                    self.value(u.user_value);
                    return;
                };
                let n = next.header();
                let f = (*n).flags;
                if !is_white(f) {
                    return;
                }
                if self.leaf_hdr != 0 {
                    self.count_leaf(n);
                }
                if let Some(s) = &mut self.sizer {
                    s.marked += s.size(n);
                }
                (*n).flags = (*n).with_slow((f & !WHITE_BITS) | BLACK);
                h = n;
            }
        }
    }

    /// [`Self::mark`] of each string of `strs`: with nothing to count per
    /// object but 5.5's sizes, a white one turns black and readable only
    /// through the slow path, as `header` leaves it.
    pub(crate) fn mark_strings<'a>(&mut self, strs: impl Iterator<Item = &'a Gc<LuaStr>>) {
        #[cfg(feature = "gc-verify")]
        let plain = self.verify.is_none();
        #[cfg(not(feature = "gc-verify"))]
        let plain = true;
        if !plain || self.leaf_hdr != 0 {
            for &s in strs {
                self.mark(s);
            }
            return;
        }
        for &s in strs {
            let h = s.header();
            // SAFETY: a handle's string is allocated and nothing is freed while marking; only its flag byte is touched
            unsafe {
                let f = (*h).flags;
                if is_white(f) {
                    if let Some(z) = &mut self.sizer {
                        z.marked += z.size(h);
                    }
                    (*h).flags = (f & !WHITE_BITS) | BLACK | SLOW;
                }
            }
        }
    }

    /// Whether `h` is an object of luna's own that PUC does not have: a
    /// native standing for one of PUC's light C functions (from 5.2 on), a
    /// table of luna's own, a userdata standing for PUC's light userdata.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    #[inline]
    pub(in crate::runtime::heap) unsafe fn luna_only(&self, h: *mut GcHeader) -> bool {
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            match (*h).tag {
                ObjTag::Native => {
                    (*h).cls & LUNA_ONLY != 0
                        || self.dialect != crate::runtime::table::Dialect::L51
                            && pace::native_upvals(h) == 0
                }
                ObjTag::Table => (*h).cls & UNLISTED != 0,
                // the closure standing for PUC's parser's (`reader_layers.rs`)
                ObjTag::Closure => (*h).cls & LUNA_ONLY != 0,
                ObjTag::Userdata => {
                    is_uncounted(h)
                        && matches!((*(h as *const Userdata)).payload, UserdataPayload::Empty)
                }
                _ => false,
            }
        }
    }

    /// `v`, held by an object of PUC's where PUC holds nothing (an upvalue
    /// of luna's own): marked with the objects of luna's own.
    pub(crate) fn quiet_value(&mut self, v: Value) {
        if self.quiet_mode {
            self.value(v);
            return;
        }
        let h = match v {
            Value::Table(t) => t.header(),
            Value::Closure(c) => c.header(),
            Value::Native(n) => n.header(),
            Value::Coro(c) => c.header(),
            Value::Userdata(u) => u.header(),
            Value::Str(s) => {
                self.mark(s);
                return;
            }
            _ => return,
        };
        // SAFETY: a value's object is allocated while it is marked; only
        // its flag byte is written
        unsafe {
            let f = (*h).flags;
            if is_white(f) {
                (*h).flags = (*h).with_slow(f & !WHITE_BITS);
                self.quiet.push(h);
            }
        }
    }

    /// Mark what the objects of luna's own hold, counting nothing: after
    /// the marking PUC counts, before the weak tables are cleared.
    pub(in crate::runtime::heap) fn drain_quiet(&mut self) {
        if self.quiet.is_empty() {
            return;
        }
        self.quiet_mode = true;
        while let Some(h) = self.quiet.pop() {
            // SAFETY: a queued object is gray and allocated until the sweep
            unsafe { blacken(h, self) };
            drain_marker(self);
        }
        self.quiet_mode = false;
    }
}

/// How many of `n`'s upvalues PUC's function has (the first ones).
pub(crate) fn native_puc_upvals(n: &crate::runtime::NativeClosure) -> usize {
    // SAFETY: `n` is an allocated native, whose header leads it
    unsafe { pace::native_upvals(n as *const _ as *mut GcHeader) }
}
