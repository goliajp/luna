//! The accounting trace's lines about the heap itself (`gc-trace`).

use super::*;

impl Heap {
    /// `R38_PROP`: how many objects PUC's list holds as the sweep finds it.
    pub(crate) fn trace_listed(&mut self) {
        if !self.trace.prop {
            return;
        }
        let mut n = 0i64;
        let mut by = [0i64; 9];
        let mut cur = self.all;
        // SAFETY: the links of `all` lead to live objects
        unsafe {
            while !cur.is_null() {
                if self.listed(cur) {
                    n += 1;
                    by[(*cur).tag as usize] += 1;
                } else if (*cur).tag == ObjTag::Ghost {
                    let g = Self::ghost_objects(&*(cur as *const Ghost));
                    n += g;
                    by[8] += g;
                }
                cur = (*cur).next;
            }
        }
        if let Some(e) = &self.puc.env51_box {
            n += e.live();
        }
        self.trace.line(format_args!("n {n} {by:?}"));
    }

    /// `R38_KEYS`: key `k` enters table `t`'s hash part.
    pub(crate) fn trace_new_key(&mut self, t: &crate::runtime::Table, k: crate::runtime::Value) {
        use crate::runtime::Value;
        if !self.trace.keys {
            return;
        }
        let key = match k {
            Value::Str(s) if s.is_empty() => String::from("-"),
            Value::Str(s) => s.as_bytes().iter().map(|b| format!("{b:02x}")).collect(),
            Value::Int(i) => format!("#{i}"),
            Value::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("#{}", f as i64),
            Value::Float(f) => format!("#{f}"),
            other => format!("?{}", other.type_name()),
        };
        self.trace.line(format_args!(
            "K {:p} {key}",
            t as *const crate::runtime::Table
        ));
    }

    /// `R38_SWEEP`: the sweep goes over `h`.
    ///
    /// # Safety
    /// `h` heads a live object of this heap.
    pub(crate) unsafe fn trace_swept(&mut self, h: *mut GcHeader, dead: bool) {
        if !self.trace.sweep {
            return;
        }
        // SAFETY: the caller's contract
        unsafe {
            let dead = u8::from(dead);
            if self.listed(h) && (*h).tag == ObjTag::Str {
                let b = string::bytes_of(h as *const LuaStr);
                let b = String::from_utf8_lossy(&b[..b.len().min(24)])
                    .escape_debug()
                    .to_string();
                self.trace.line(format_args!("w 0 {dead} {b}"));
            } else if self.listed(h) {
                let tag = (*h).tag as u8;
                self.trace.line(format_args!("w {tag} {dead}"));
            } else if (*h).tag == ObjTag::Ghost && (*h).sub == 0 {
                let n = Self::ghost_objects(&*(h as *const Ghost));
                self.trace.line(format_args!("w g{n} {dead}"));
            }
        }
    }

    /// `R38_REFUSE` refuses a table resize from `old_a` array slots to
    /// `new_a` and `new_h` nodes: one of the parts PUC makes is refused.
    pub(crate) fn refuse_table(&mut self, old_a: usize, new_a: usize, new_h: usize) -> bool {
        let (oa, na, nh) = (
            self.array_bytes(old_a),
            self.array_bytes(new_a),
            self.hash_bytes(new_h),
        );
        self.trace.refuse_now(oa, na) || self.trace.refuse_now(0, nh)
    }

    /// `R38_REFUSE` refuses a new long string of `len` bytes.
    pub(crate) fn refuse_string(&mut self, len: usize) -> bool {
        let size = self.puc.sizes.string(
            len,
            self.table_dialect == crate::runtime::table::Dialect::L55,
        );
        len > crate::runtime::string::MAX_SHORT_LEN && self.trace.refuse_now(0, size)
    }

    /// `R38_TRACE`: a thread of `stack` slots shrunk with `inuse` in use.
    pub(crate) fn trace_shrink(&mut self, stack: u32, inuse: u32) {
        self.trace.line(format_args!("sh {stack} {inuse}"));
    }
}
