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
        for (i, n) in self.nodes.iter().enumerate() {
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
}
