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
        if let Some(e) = &self.env51_box {
            n += e.live();
        }
        self.trace.line(format_args!("n {n} {by:?}"));
    }

    /// `R38_TRACE`: a thread of `stack` slots shrunk with `inuse` in use.
    pub(crate) fn trace_shrink(&mut self, stack: u32, inuse: u32) {
        self.trace.line(format_args!("sh {stack} {inuse}"));
    }
}
