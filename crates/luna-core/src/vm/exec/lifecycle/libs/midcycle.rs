//! An embedder's opening that leaves PUC's collector in the middle of a
//! cycle: luna's collector put where the record says PUC's stands, each
//! object the record names found by its path from the registry.

use super::*;
use crate::runtime::heap::GcHeader;

impl Vm {
    /// The collector as `rec` (`puc_libs::midcycle`) says the opening left
    /// it, when that is in the propagation of a cycle; false (nothing done)
    /// otherwise.
    pub(super) fn libs_midcycle(&mut self, rec: &[&str]) -> bool {
        let Some(head) = rec.first().and_then(|l| l.strip_prefix("S ")) else {
            return false;
        };
        let w: Vec<i64> = head.split(' ').filter_map(|x| x.parse().ok()).collect();
        let &[state, debt, estimate, traversed] = w.as_slice() else {
            return false;
        };
        // the propagation (5.2's `GCSpropagate`)
        if state != 0 {
            return false;
        }
        let (mut black, mut gray, mut again) = (Vec::new(), Vec::new(), Vec::new());
        for line in &rec[1..] {
            let (kind, path) = line.split_at(2);
            let Some(h) = self.object_at(path) else {
                debug_assert!(false, "no object at {path}");
                continue;
            };
            match kind {
                "B " => black.push(h),
                "G " => gray.push(h),
                _ => again.push(h),
            }
        }
        // SAFETY: each header is of an object the registry or the strings
        // hold, and the collector is in the pause after the opening
        unsafe { self.heap.acct_enter_midcycle(&black, &gray, &again) };
        let d = self.heap.table_dialect;
        let bytes = self.heap.bytes();
        let p = &mut self.heap.pace;
        p.state = crate::runtime::gc_pace::St::Propagate;
        p.estimate = estimate;
        p.trav = traversed;
        p.set_debt(d, bytes, debt);
        self.heap.refresh_gc_limit();
        true
    }

    /// The object a record's path names: from the registry (`R`), the main
    /// thread (`L`) or a basic type's metatable (`T<tag>#mt`), through
    /// `.key`, `[i]`, `#mt`, `#uv` and `^i` (a C function's upvalue); or a
    /// string by its bytes (`S:` and hex).
    fn object_at(&self, path: &str) -> Option<*mut GcHeader> {
        if let Some(hex) = path.strip_prefix("S:") {
            return self.string_at(hex);
        }
        let (mut v, mut rest) = if let Some(r) = path.strip_prefix('R') {
            (Value::Table(self.registry?), r)
        } else if let Some(r) = path.strip_prefix('L') {
            (Value::Coro(self.main_coro?), r)
        } else {
            let r = path.strip_prefix('T')?;
            let end = r.find('#')?;
            // PUC's tags: 0 nil, 1 boolean, 2 light userdata, 3 number,
            // 4 string, 6 function, 8 thread
            let slot = match r[..end].parse::<u32>().ok()? {
                0 => 0,
                1 => 1,
                2 => 5,
                3 => 2,
                4 => 3,
                6 => 4,
                8 => 6,
                _ => return None,
            };
            (
                Value::Table(self.type_mt[slot]?),
                r[end..].strip_prefix("#mt")?,
            )
        };
        while !rest.is_empty() {
            let at = rest[1..]
                .find(['.', '[', '#', '^'])
                .map_or(rest.len(), |i| i + 1);
            let (seg, tail) = rest.split_at(at);
            rest = tail;
            v = match (seg.as_bytes()[0], v) {
                (b'.', Value::Table(t)) => {
                    t.get(Value::Str(self.heap.find_interned(&seg.as_bytes()[1..])?))
                }
                (b'[', Value::Table(t)) => t.get(Value::Int(seg[1..seg.len() - 1].parse().ok()?)),
                (b'#', Value::Table(t)) if seg == "#mt" => Value::Table(t.metatable()?),
                (b'#', Value::Userdata(u)) if seg == "#mt" => Value::Table(u.metatable()?),
                (b'#', Value::Userdata(u)) if seg == "#uv" => u.user_value,
                (b'^', Value::Native(n)) => *n.upvals.get(seg[1..].parse::<usize>().ok()? - 1)?,
                _ => return None,
            };
        }
        header_of(v)
    }

    /// The string of the bytes `hex` spells: luna's interned one, or for a
    /// long one the package path it stands for.
    fn string_at(&self, hex: &str) -> Option<*mut GcHeader> {
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<_, _>>()
            .ok()?;
        if let Some(s) = self.heap.find_interned(&bytes) {
            return header_of(Value::Str(s));
        }
        // PUC's default search paths, which luna's own stand for
        let name: &[u8] = if bytes.windows(5).any(|w| w == b"?.lua") {
            b"path"
        } else {
            b"cpath"
        };
        let loaded = self
            .registry?
            .get(Value::Str(self.heap.find_interned(b"_LOADED")?));
        let Value::Table(loaded) = loaded else {
            return None;
        };
        let Value::Table(pkg) = loaded.get(Value::Str(self.heap.find_interned(b"package")?)) else {
            return None;
        };
        header_of(pkg.get(Value::Str(self.heap.find_interned(name)?)))
    }
}

/// The header of the object `v` holds.
fn header_of(v: Value) -> Option<*mut GcHeader> {
    Some(match v {
        Value::Str(s) => s.as_ptr() as *mut GcHeader,
        Value::Table(t) => t.as_ptr() as *mut GcHeader,
        Value::Closure(c) => c.as_ptr() as *mut GcHeader,
        Value::Native(n) => n.as_ptr() as *mut GcHeader,
        Value::Coro(c) => c.as_ptr() as *mut GcHeader,
        Value::Userdata(u) => u.as_ptr() as *mut GcHeader,
        _ => return None,
    })
}
