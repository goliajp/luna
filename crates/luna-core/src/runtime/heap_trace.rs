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

impl Heap {
    /// Each object's place on the list the collector sweeps (counting only
    /// PUC's), and the place the sweep goes on from (`None`: not
    /// sweeping), for comparing the collector's state with PUC's.
    pub(crate) fn trace_places(&self) -> (std::collections::HashMap<usize, usize>, Option<usize>) {
        let mut at = std::collections::HashMap::new();
        let mut sweep = None;
        let mut cur = self.all;
        let mut i = 0;
        if self.phase == GcPhase::Sweep && self.sweep_prev.is_null() {
            sweep = Some(0);
        }
        // SAFETY: the links of `all` lead to live objects; only headers
        // are read
        unsafe {
            while !cur.is_null() {
                if self.listed(cur) {
                    at.insert(cur as usize, i);
                    i += 1;
                }
                if cur == self.sweep_prev {
                    sweep = Some(i);
                }
                cur = (*cur).next;
            }
        }
        (at, sweep)
    }

    /// The colour of the object behind `h`, as PUC's dump names it: `B`
    /// black, `G` gray, `W0` the current white, `W1` the other.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(crate) unsafe fn trace_colour(&self, h: *mut GcHeader) -> &'static str {
        // SAFETY: the caller's contract
        let f = unsafe { (*h).flags };
        if is_black(f) {
            "B"
        } else if !is_white(f) {
            "G"
        } else if f & self.current_white != 0 {
            "W0"
        } else {
            "W1"
        }
    }
}
