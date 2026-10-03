//! Weak tables: mode lookup and clearing after marking.

use super::*;

impl Table {
    /// `(weak_keys, weak_values)` from the metatable's `__mode` field. Read by
    /// scanning the metatable for the `__mode` string (no interned key needed
    /// inside the collector).
    pub(crate) fn weak_mode(&self) -> (bool, bool) {
        let Some(mt) = self.metatable else {
            return (false, false);
        };
        for n in mt.nodes().iter() {
            if let (Value::Str(k), Value::Str(mode)) = (n.key(), n.val)
                && k.as_bytes() == b"__mode"
            {
                let b = mode.as_bytes();
                return (b.contains(&b'k'), b.contains(&b'v'));
            }
        }
        (false, false)
    }

    /// True when this table holds at least one direct reference (array slot,
    /// hash key, or hash value) to a coroutine whose mark bit is still clear.
    /// Used by the GC's cycle-finalize check (PUC 5.3 gc.lua :502) to detect
    /// the table ↔ thread reference cycle that needs an extra GC round before
    /// `__gc` runs. Tag-level scan avoids walking the full reference graph.
    pub(crate) fn refs_contain_unmarked_coro(&self) -> bool {
        use crate::runtime::heap::header_is_marked;
        let atags = self.atags();
        let avals = self.avals();
        for (i, &tag) in atags.iter().enumerate() {
            if tag == raw::CORO {
                // SAFETY: the tag at this index is CORO, so the `co` field of the payload is the pointer of a coroutine this live table holds
                let co = unsafe { Gc::from_ptr_unchecked(avals[i].co) };
                if !header_is_marked(co) {
                    return true;
                }
            }
        }
        for n in self.nodes().iter() {
            if let Value::Coro(co) = n.key()
                && !header_is_marked(co)
            {
                return true;
            }
            if let Value::Coro(co) = n.val
                && !header_is_marked(co)
            {
                return true;
            }
        }
        false
    }

    /// Clear entries whose weak key/value did not survive marking. `is_dead`
    /// reports whether a GC value was left unmarked (about to be swept).
    /// Clear weak-table entries whose key/value no longer carries a live
    /// reference. `is_dead` is a **pure** check (no side effects); the GC
    /// uses `mark_string` to resurrect any string that's still reachable via
    /// a *surviving* entry — Lua manual §2.5.4 says strings in weak tables
    /// are not collected as long as their entry is, and PUC `iscleared`
    /// implements that by marking the string during the same scan.
    pub(crate) fn clear_weak(
        &mut self,
        wk: bool,
        wv: bool,
        is_dead: &dyn Fn(Value) -> bool,
        mark_string: &dyn Fn(Value),
    ) {
        if wv {
            let n = self.asize as usize;
            for i in 0..n {
                let tag = self.atags()[i];
                if raw::is_gc(tag) {
                    // SAFETY: `tag` and the raw value come from this table's parallel `atags` / `avals` arrays, which the table writers always keep in sync — the tag byte matches the raw payload's discriminator (see `runtime::value` `raw` module).
                    let v = unsafe { Value::pack(tag, self.avals()[i]) };
                    if is_dead(v) {
                        self.atags_mut()[i] = raw::NIL;
                        self.avals_mut()[i] = RawVal::NIL;
                        self.note_atag_change(i, tag, raw::NIL);
                    } else {
                        mark_string(v);
                    }
                }
            }
        }
        for n in self.nodes_mut().iter_mut() {
            if n.val.is_nil() {
                // PUC `clearbykeys`/`clearbyvalues` end with
                // `if (isempty(gval(n))) clearkey(n)`: an EMPTY entry's
                // collectable key must be demoted to a dead key. A
                // tombstone (`t[k] = nil` leaves val nil, key kept for
                // chain links) is otherwise invisible to this sweep AND
                // unmarked by the weak-key trace, so its string key gets
                // freed while `find_node` still raw_eq's it walking the
                // chain (a use-after-free ASAN reports on Linux).
                if !n.dead_key
                    && matches!(
                        n.key(),
                        Value::Table(_)
                            | Value::Closure(_)
                            | Value::Native(_)
                            | Value::Coro(_)
                            | Value::Userdata(_)
                            | Value::Str(_)
                    )
                {
                    n.key_tag = crate::runtime::value::tag::NIL;
                    n.dead_key = true;
                }
                continue;
            }
            let key_dead = wk && is_dead(n.key());
            let val_dead = wv && is_dead(n.val);
            if key_dead || val_dead {
                // entry removed. PUC `setdeadkey`: when the key was a
                // collectable, drop the Gc pointer so a later raw_eq cannot
                // spuriously match a new object that gets allocated at the
                // same freed address. Keep `next` so the chain back-links
                // through this node still reach downstream entries; the
                // `dead_key` flag tells `find_node` to skip the comparison
                // and `insert_new` to treat the slot as a free
                // main-position owner that may inherit the chain.
                n.val = Value::Nil;
                if matches!(
                    n.key(),
                    Value::Table(_)
                        | Value::Closure(_)
                        | Value::Native(_)
                        | Value::Coro(_)
                        | Value::Userdata(_)
                        | Value::Str(_)
                ) {
                    n.key_tag = crate::runtime::value::tag::NIL;
                    n.dead_key = true;
                }
            } else {
                // entry survives — resurrect any string reachable through it
                if wk {
                    mark_string(n.key());
                }
                if wv {
                    mark_string(n.val);
                }
            }
        }
    }
}
