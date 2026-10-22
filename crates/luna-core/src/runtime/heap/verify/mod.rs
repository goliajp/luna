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

mod freed;
pub(crate) use freed::dead_child;
