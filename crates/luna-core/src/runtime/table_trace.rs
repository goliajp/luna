//! Marking a table's contents for the collector (PUC `traversetable`).
//!
//! The loops read each slot's tag byte and, for a collectable, its payload
//! word as the object pointer, without building a `Value`: every
//! collectable variant keeps a pointer to an object that starts with its
//! `GcHeader`, and the collectable tags are one contiguous range in both
//! encodings.

use super::{Node, Table};
use crate::runtime::heap::{GcHeader, Marker};
use crate::runtime::value::{Value, raw, tag};

// the range checks below rely on the collectable tags being exactly
// `STR..=USERDATA` in both encodings, with light userdata outside it
const _: () = {
    assert!(tag::USERDATA - tag::STR == 5 && tag::LIGHTUSERDATA == tag::USERDATA + 1);
    assert!(raw::USERDATA - raw::STR == 5 && raw::LIGHTUSERDATA == raw::USERDATA + 1);
};

/// True when a `Value` tag names a collectable object.
#[inline(always)]
fn value_tag_is_gc(t: u8) -> bool {
    t.wrapping_sub(tag::STR) <= tag::USERDATA - tag::STR
}

/// The object of a collectable `Value`.
///
/// # Safety
/// `v`'s tag is collectable (`value_tag_is_gc`).
#[inline(always)]
unsafe fn value_obj(v: &Value) -> *mut GcHeader {
    // SAFETY: `Value` is `repr(C, u8)` with its payload at offset 8; a
    // collectable variant's payload is a `Gc<T>`, a non-null pointer to an
    // object whose first field is its `GcHeader`
    unsafe { *((v as *const Value as *const u8).add(8) as *const *mut GcHeader) }
}

/// The object of a node's collectable key.
///
/// # Safety
/// `n.key_tag` is collectable.
#[inline(always)]
unsafe fn key_obj(n: &Node) -> *mut GcHeader {
    // SAFETY: `set_key` stores a key's payload as a `Value` has it, so a
    // collectable key's payload word is its object pointer
    unsafe { *(n.key_payload.as_ptr() as *const *mut GcHeader) }
}

impl Table {
    pub(crate) fn trace(&self, m: &mut Marker) {
        let (wk, wv) = match self.metatable {
            Some(_) => self.weak_mode(),
            None => (false, false),
        };
        if !wk && !wv {
            self.mark_array(m);
            self.mark_nodes::<true, true>(m);
        } else {
            self.trace_weak(wk, wv, m);
        }
        if let Some(mt) = self.metatable {
            m.header(mt.as_ptr() as *mut GcHeader);
        }
    }

    fn trace_weak(&self, wk: bool, wv: bool, m: &mut Marker) {
        m.weak.push(self as *const Table as *mut Table);
        // weak keys + strong values = an ephemeron table: its hash values are
        // marked only if the key proves reachable (deferred to the convergence
        // pass), not here. PUC 5.1 predates ephemerons — under `no_ephemeron`
        // a weak-key table marks its values strongly during this pass, which
        // is what gc.lua's "weak tables" section requires.
        let ephemeron = wk && !wv && !m.no_ephemeron;
        if ephemeron {
            m.ephemeron.push(self as *const Table as *mut Table);
        }
        // array keys are integers (never weakly collected); skip values only
        // when the table has weak values
        if !wv {
            self.mark_array(m);
        }
        match (!wk, !wv && !ephemeron) {
            (true, true) => self.mark_nodes::<true, true>(m),
            (true, false) => self.mark_nodes::<true, false>(m),
            (false, true) => self.mark_nodes::<false, true>(m),
            (false, false) => {}
        }
    }

    #[inline(always)]
    fn mark_array(&self, m: &mut Marker) {
        for (&t, v) in self.atags().iter().zip(self.avals()) {
            if raw::is_gc(t) {
                // SAFETY: the tag and payload arrays are kept in step by
                // every writer; a collectable tag's payload is the object
                // pointer, whichever pointer field of the union holds it
                m.header(unsafe { v.s } as *mut GcHeader);
            }
        }
    }

    #[inline(always)]
    fn mark_nodes<const KEYS: bool, const VALS: bool>(&self, m: &mut Marker) {
        for n in self.nodes().iter() {
            // a dead key's tag is nil, so it is skipped here
            if KEYS && value_tag_is_gc(n.key_tag) {
                // SAFETY: the tag was just checked
                m.header(unsafe { key_obj(n) });
            }
            if VALS && value_tag_is_gc(n.val.tag_byte()) {
                // SAFETY: the tag was just checked
                m.header(unsafe { value_obj(&n.val) });
            }
        }
    }

    /// Ephemeron pass: mark the value of every hash entry whose key is alive
    /// (`alive` decides — strong/marked keys, plus strings/numbers which are
    /// never weakly collected). Returns true if any value was newly marked, so
    /// the caller can iterate to a fixpoint (PUC `traverseephemeron`).
    pub(crate) fn converge_ephemeron(&self, alive: &dyn Fn(Value) -> bool, m: &mut Marker) -> bool {
        let mut changed = false;
        for n in self.nodes().iter() {
            if !n.val.is_nil() && alive(n.key()) {
                changed |= m.value(n.val);
            }
        }
        changed
    }
}

#[cfg(test)]
#[path = "table_trace_tests.rs"]
mod tests;
