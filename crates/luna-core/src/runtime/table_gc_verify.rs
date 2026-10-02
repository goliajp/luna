//! gc-verify probe for hash lookups.

use super::*;

impl Table {
    /// Read-time probe: the query key and every node key a hash lookup
    /// compares must be live. All hash lookups meet in `find_node`, so a
    /// dangling string is named at its dereference site with its role.
    pub(super) fn verify_find_node_keys(&self, k: Value) {
        let hdr = |v: Value| -> Option<usize> {
            match v {
                Value::Str(s) => Some(s.as_ptr() as usize),
                Value::Table(t) => Some(t.as_ptr() as usize),
                _ => None,
            }
        };
        if let Some(p) = hdr(k)
            && crate::runtime::gc_verify_probe::is_freed(p)
        {
            panic!("[gc-verify] find_node QUERY key {p:#x} is freed (dangling)");
        }
        for (i, n) in self.nodes().iter().enumerate() {
            // NOTE: tombstones (val nil, key kept) are NOT skipped —
            // the walk below raw_eq's their keys too.
            if n.dead_key {
                continue;
            }
            if let Some(p) = hdr(n.key())
                && crate::runtime::gc_verify_probe::is_freed(p)
            {
                panic!(
                    "[gc-verify] find_node NODE key {p:#x} (slot {i}, \
                         tombstone {}, table {:#x}) is freed (dangling)",
                    n.val.is_nil(),
                    self as *const Table as usize
                );
            }
        }
    }

    /// `gc-verify`: after a completed sweep, every collectable
    /// reference this table still holds (array values, node keys/values,
    /// metatable) must point at a live heap object. Nodes flagged
    /// `dead_key` are the sanctioned exception — their key pointer is
    /// documented-dangling and never dereferenced. `describe` receives
    /// (what, node-index, tag-byte, ptr) on violation.
    pub(crate) fn verify_refs(
        &self,
        is_live: &dyn Fn(Value) -> bool,
        report: &dyn Fn(&str, usize, Value),
    ) {
        let atags = self.atags();
        let avals = self.avals();
        for (i, &tag) in atags.iter().enumerate() {
            if raw::is_gc(tag) {
                // SAFETY: tags/vals parallel arrays kept in sync by all table writers.
                let v = unsafe { Value::pack(tag, avals[i]) };
                if !is_live(v) {
                    report("array value", i, v);
                }
            }
        }
        for (i, n) in self.nodes().iter().enumerate() {
            if n.val.is_nil() {
                continue;
            }
            if !n.dead_key && !is_live(n.key()) {
                report("node key", i, n.key());
            }
            if !is_live(n.val) {
                report("node value", i, n.val);
            }
        }
        if let Some(mt) = self.metatable
            && !is_live(Value::Table(mt))
        {
            report("metatable", 0, Value::Table(mt));
        }
    }
}
