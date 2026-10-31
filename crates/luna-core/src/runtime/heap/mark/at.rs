//! Marking an object with what it holds as soon as it is marked, or
//! later: the objects PUC traverses at once, and luna's own objects,
//! which PUC does not have, marked after the counted marking without
//! taking part in its order or its work.

use super::*;

impl Marker {
    /// The model's work for marking the leaf `h` (5.2 and 5.3), out of
    /// line: the other dialects' marking does not call it.
    #[cold]
    #[inline(never)]
    pub(super) fn count_leaf(&mut self, h: Gc<GcHeader>) {
        self.leaf_bytes += self.rules.leaf_work(h);
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
                self.mark_env51(h, (*h).cls);
                if let UserdataPayload::Host {
                    data,
                    trace_fn: Some(trace_fn),
                    ..
                } = &u.payload
                {
                    let mut um = crate::runtime::userdata::UserdataMarker::__new_internal(self);
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
                if self.count_leaves {
                    self.count_leaf(next.hdr());
                }
                if let Some(s) = &mut self.sizer {
                    s.marked += s.size(n);
                }
                (*n).set_flags((f & !WHITE_BITS) | BLACK);
                h = n;
            }
        }
    }

    /// [`Self::mark`] of each string of `strs`: with nothing to count per
    /// object but 5.5's sizes, a white one turns black and readable only
    /// through the slow path, as `header` leaves it.
    pub(crate) fn mark_strings<'a>(&mut self, strs: impl Iterator<Item = &'a Gc<LuaStr>>) {
        let plain = !self.verify.active();
        if !plain || self.count_leaves {
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
                    (*h).blacken_from(f);
                    model::string_held(&mut *h);
                }
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
        // left white until the counted marking is done, which may reach it
        // first and count it: tagged in the low bit, which a header's
        // alignment leaves free
        // SAFETY: a value's object is allocated while it is marked; only
        // its flag byte is read
        if unsafe { is_white((*h).flags) } {
            self.quiet.push(h.map_addr(|a| a | 1));
        }
    }

    /// Mark what the objects of luna's own hold, counting nothing: after
    /// the marking PUC counts, before the weak tables are cleared.
    pub(in crate::runtime) fn drain_quiet(&mut self) {
        if !self.quiet_mode && self.quiet_parse_trees() {
            while let Some(h) = self.pop() {
                // SAFETY: a gray object is allocated until the sweep
                unsafe { blacken(h, self) };
            }
            self.quiet_mode = false;
        }
        if self.quiet.is_empty() {
            return;
        }
        self.quiet_mode = true;
        while let Some(h) = self.quiet.pop() {
            let root = h.addr() & 1 != 0;
            let h = h.map_addr(|a| a & !1);
            // SAFETY: a queued object is allocated until the sweep; one
            // held by luna alone is gray, one of its roots marked only if
            // still white
            unsafe {
                if root {
                    let f = (*h).flags;
                    if !is_white(f) {
                        continue;
                    }
                    (*h).set_flags(f & !WHITE_BITS);
                }
                blacken(h, self);
            }
            drain_marker(self);
        }
        self.quiet_mode = false;
    }
}
