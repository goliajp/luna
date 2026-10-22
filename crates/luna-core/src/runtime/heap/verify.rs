//! `gc-verify` diagnostics of the collector: the live set and the checks
//! that name a missed barrier or a dangling reference at the collect that
//! created it.

use super::*;
use crate::runtime::dialect::Dialect;

impl Heap {
    /// `gc-verify` — the set of every live object header
    /// (all + fixed + finalizer queues), for callers that need to
    /// audit their own containers (e.g. the VM auditing its register
    /// stack after a collect).
    pub fn debug_live_set(&self) -> std::collections::HashSet<usize> {
        let mut live = std::collections::HashSet::new();
        // SAFETY: heap-owned intrusive lists; all elements are live
        // allocations.
        unsafe {
            for mut cur in [self.all, self.fixed] {
                while !cur.is_null() {
                    live.insert(cur as usize);
                    cur = (*cur).next;
                }
            }
        }
        for &h in &self.finalize {
            live.insert(h as usize);
        }
        for &h in self.tobefnz_live() {
            live.insert(h as usize);
        }
        live
    }

    /// See call site in [`Heap::atomic_tail`]. Panics on the first
    /// BLACK table whose collectable child is dead-white (missed
    /// barrier — that child is about to be swept while still
    /// referenced).
    pub(crate) fn verify_tricolor(&self, ctx: &str) {
        let new_white = self.current_white;
        // SAFETY: `all` is the heap's own intrusive list; every element
        // is a live allocation (nothing has been freed this cycle yet).
        unsafe {
            let is_live = |v: Value| -> bool {
                let h = match v {
                    Value::Str(s) => s.as_ptr() as *mut GcHeader,
                    Value::Table(t) => t.as_ptr() as *mut GcHeader,
                    Value::Closure(c) => c.as_ptr() as *mut GcHeader,
                    Value::Native(n) => n.as_ptr() as *mut GcHeader,
                    Value::Coro(c) => c.as_ptr() as *mut GcHeader,
                    Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
                    _ => return true,
                };
                let f = (*h).flags;
                !(is_white(f) && (f & new_white) == 0)
            };
            let mut cur = self.all;
            while !cur.is_null() {
                let f = (*cur).flags;
                if (*cur).tag == ObjTag::Table && (f & BLACK) != 0 {
                    let t = &*(cur as *const Table);
                    // Weak tables may legitimately reference dead objects
                    // between clear passes; they were just cleared above,
                    // but their sanctioned dead_key nodes are skipped by
                    // verify_refs anyway. Only strong tables assert.
                    let (wk, wv) = t.weak_mode();
                    if wk || wv {
                        cur = (*cur).next;
                        continue;
                    }
                    let tp = cur as usize;
                    t.verify_refs(&is_live, &|what, idx, v| {
                        // Pre-sweep: dereferencing v is still safe — name
                        // the key when it is a string.
                        let detail = match v.as_bytes() {
                            Some(b) => format!("str {:?}", String::from_utf8_lossy(b)),
                            None => format!(
                                "non-str {:#x}",
                                match v {
                                    Value::Table(t) => t.as_ptr() as usize,
                                    Value::Closure(c) => c.as_ptr() as usize,
                                    Value::Coro(c) => c.as_ptr() as usize,
                                    Value::Userdata(u) => u.as_ptr() as usize,
                                    Value::Native(n) => n.as_ptr() as usize,
                                    _ => 0,
                                }
                            ),
                        };
                        panic!(
                            "[gc-verify] {ctx}: BLACK table {tp:#x} holds dead-white {what} \
                             (slot {idx}): {detail} — missed write barrier"
                        );
                    });
                }
                cur = (*cur).next;
            }
        }
    }

    /// `gc-verify` — post-sweep dangling-reference check
    /// (PUC `lua_checkmemory` analogue). Builds the live set from the
    /// `all` list + finalizer queues, then walks every live table's
    /// collectable refs via [`Table::verify_refs`]. Panics with table
    /// pointer + slot detail on the first reference whose target is no
    /// longer live — i.e. at the collect that CREATED the dangling
    /// pointer, not at the later allocator-dependent dereference.
    pub fn verify_no_dangling(&self, ctx: &str) {
        use std::collections::HashSet;
        self.verify_kept(ctx);
        fn value_header(v: Value) -> Option<*mut GcHeader> {
            match v {
                Value::Str(s) => Some(s.as_ptr() as *mut GcHeader),
                Value::Table(t) => Some(t.as_ptr() as *mut GcHeader),
                Value::Closure(c) => Some(c.as_ptr() as *mut GcHeader),
                Value::Native(n) => Some(n.as_ptr() as *mut GcHeader),
                Value::Coro(c) => Some(c.as_ptr() as *mut GcHeader),
                Value::Userdata(u) => Some(u.as_ptr() as *mut GcHeader),
                _ => None,
            }
        }
        let mut live: HashSet<usize> = HashSet::new();
        // SAFETY: `all` / `fixed` / finalizer-queue pointers are the
        // heap's own intrusive lists; every element is a live allocation
        // until free_obj unlinks it.
        unsafe {
            for mut cur in [self.all, self.fixed] {
                while !cur.is_null() {
                    live.insert(cur as usize);
                    cur = (*cur).next;
                }
            }
            for &h in &self.finalize {
                live.insert(h as usize);
            }
            for &h in self.tobefnz_live() {
                live.insert(h as usize);
            }
            let is_live = |v: Value| -> bool {
                match value_header(v) {
                    None => true,
                    Some(h) => live.contains(&(h as usize)),
                }
            };
            // `all` holds the swept survivors and, during a sweep, the
            // part not swept yet; a dangling reference can live in either
            // while the mutator runs between sweep steps.
            let new_white = self.current_white;
            for head in [self.all] {
                let mut cur = head;
                while !cur.is_null() {
                    // Skip dead objects not swept yet:
                    // they are unreachable, so a dangling ref inside them
                    // is benign (their peers may already be freed).
                    let f = (*cur).flags;
                    if is_white(f) && (f & new_white) == 0 {
                        cur = (*cur).next;
                        continue;
                    }
                    if (*cur).tag == ObjTag::Table {
                        let t = &*(cur as *const Table);
                        let tp = cur as usize;
                        t.verify_refs(&is_live, &|what, idx, v| {
                            // Do NOT Debug-format `v` — for Value::Str that
                            // would dereference the (dangling) payload.
                            let p = value_header(v).map(|h| h as usize).unwrap_or(0);
                            panic!(
                                "[gc-verify] {ctx}: table {tp:#x} holds dangling {what} \
                                 (slot {idx}, target {p:#x} {:?})",
                                crate::runtime::gc_verify_probe::freed_str(p)
                            );
                        });
                    }
                    cur = (*cur).next;
                }
            }
        }
    }

    /// `gc-verify`, once marking is done: no black object holds an object
    /// the sweep will free (a missed write barrier), whatever its kind.
    /// Panics naming both on the first.
    pub(crate) fn verify_black_children(&self, ctx: &'static str) {
        let mut m = Marker {
            stack: Default::default(),
            overflow: false,
            scan: std::ptr::null_mut(),
            weak: Vec::new(),
            ephemeron: Vec::new(),
            no_ephemeron: !self.rules.ephemeron(),
            dialect: self.dialect(),
            cached_protos: Vec::new(),
            leaf_black: 0,
            leaf_hdr: 0,
            leaf_bytes: 0,
            leaf_upval: 0,
            sizer: None,
            quiet: Vec::new(),
            quiet_mode: false,
            env51: std::ptr::null(),
            parse: std::ptr::null(),
            stands: std::ptr::null(),
            verify: Default::default(),
        };
        let mut cur = self.all;
        // SAFETY: `all` links this heap's allocated objects, none freed before
        // the sweep; tracing with `verify` set reads the children and colors
        // nothing
        unsafe {
            while !cur.is_null() {
                if is_black((*cur).flags) {
                    m.verify.check = Some((self.current_white, cur, ctx));
                    super::mark::trace_children(cur, &mut m);
                }
                cur = (*cur).next;
            }
        }
    }
}
impl Heap {
    /// The class byte of `h`, which the sweep goes over, says whether PUC
    /// lists it (`strt`: the string table holds strings).
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(crate) unsafe fn verify_class_byte(&self, h: *mut GcHeader, strt: bool) {
        // SAFETY: the caller's contract
        unsafe {
            assert_eq!(
                self.listed(h),
                ((*h).tag != ObjTag::Ghost || super::parse::is_load_tab(h))
                    && self.puc_listed(h, strt),
                "[gc-verify] class byte of a {:?}",
                (*h).tag
            );
        }
    }

    /// Whether PUC keeps the object behind `h` on the list it sweeps
    /// (`allgc`, 5.1 `rootgc`): one of PUC's, with no finalizer pending,
    /// and (`strt`) no string the string table holds.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    unsafe fn puc_listed(&self, h: *mut GcHeader, strt: bool) -> bool {
        // SAFETY: the caller's contract
        unsafe {
            let d51 = self.dialect() == Dialect::L51;
            let own51 = match (*h).tag {
                ObjTag::Coro => false,
                ObjTag::Native => (*h).cls & LUNA_ONLY != 0,
                ObjTag::Userdata => {
                    is_uncounted(h)
                        && !matches!((*(h as *const Userdata)).payload, UserdataPayload::File(_))
                }
                _ => is_uncounted(h),
            };
            let pending51 = d51 && (*h).flags & DEFERRED != 0;
            if (if d51 { own51 } else { is_uncounted(h) })
                || (*h).flags & FIN != 0 && !d51
                || pending51
            {
                return false;
            }
            match (*h).tag {
                ObjTag::Str => {
                    !strt || (self.dialect() == Dialect::L52 && !(*(h as *const LuaStr)).is_short())
                }
                ObjTag::Table => !self.is_own_table(h as *const Table),
                _ => true,
            }
        }
    }

    /// The first object never swept (`fixedgc`).
    #[inline]
    pub(in crate::runtime) fn fixed_head(&self) -> *mut GcHeader {
        self.fixed
    }

    /// The object `h` freed: noted for the read-time probes.
    pub(crate) fn note_freed(&mut self, h: *mut GcHeader) {
        self.hooks.recently_freed.insert(h as usize);
        crate::runtime::gc_verify_probe::FREED.with(|f| f.borrow_mut().insert(h as usize));
        // SAFETY: the caller of `free_obj` names an allocated object, read
        // as a string when its tag says it is one
        let s = unsafe { ((*h).tag == ObjTag::Str).then(|| Gc::from_ptr(h as *mut LuaStr)) };
        if let Some(s) = s {
            let b = s.as_bytes().to_vec();
            crate::runtime::gc_verify_probe::FREED_STR
                .with(|f| f.borrow_mut().insert(h as usize, b));
        }
    }

    /// `gc-verify`: the address `h`, freed before, holds a new object.
    pub(in crate::runtime) fn forget_freed(&mut self, h: *mut GcHeader) {
        self.hooks.recently_freed.remove(&(h as usize));
        crate::runtime::gc_verify_probe::FREED.with(|f| f.borrow_mut().remove(&(h as usize)));
    }

    /// `gc-verify`: a new reference to the object at `h` (`what`) while a
    /// sweep runs must not be to one the sweep frees.
    pub(crate) fn verify_not_dying(&self, h: *const GcHeader, what: &str) {
        // SAFETY: the caller names an allocated object; only its flags are read
        let f = unsafe { (*h).flags };
        if self.phase == GcPhase::Sweep && is_white(f) && f & self.current_white == 0 {
            panic!(
                "[gc-verify] {what} {:#x} is dead, swept this cycle",
                h as usize
            );
        }
    }

    /// `gc-verify`: the color of the object at `h` against the heap's
    /// current white, and the collector's phase.
    pub(crate) fn describe(&self, h: *const GcHeader) -> String {
        // SAFETY: the caller names an allocated object; only its flags are read
        let f = unsafe { (*h).flags };
        format!(
            "flags {f:#x} white {:#x} phase {:?}",
            self.current_white, self.phase
        )
    }
}

/// [`Heap::verify_black_children`]'s check of child `h` of `parent`.
///
/// # Safety
/// `h` and `parent` head allocated objects.
pub(crate) unsafe fn dead_child(
    h: *mut GcHeader,
    (new_white, parent, ctx): (u8, *mut GcHeader, &'static str),
) {
    // SAFETY: the caller's contract; only headers are read
    unsafe {
        let f = (*h).flags;
        // a string only a load holds is kept by the sweep
        if is_white(f) && f & new_white == 0 && !crate::runtime::puc::verify::is_kept(h) {
            let what = if (*h).tag == ObjTag::Str {
                String::from_utf8_lossy(string::bytes_of(h as *const LuaStr)).into_owned()
            } else {
                String::new()
            };
            panic!(
                "[gc-verify] {ctx}: black {:?} {:#x} (cls {:#x}) holds {:?} {:#x} {what:?} (sub {:#x} cls {:#x}), which the sweep frees",
                (*parent).tag,
                parent as usize,
                (*parent).cls,
                (*h).tag,
                h as usize,
                (*h).sub,
                (*h).cls
            );
        }
    }
}
