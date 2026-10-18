//! Sweeping 5.1's environment records where PUC's list has them.

use super::*;

impl Heap {
    /// The sweep reaches `cur` (`after`: has swept it): the records swept
    /// with it, as PUC sweeps them. Returns how many.
    #[inline(always)]
    pub(in crate::runtime) fn env51_sweep(&mut self, cur: *mut GcHeader, after: bool) -> i64 {
        match &self.env51_box {
            Some(e) if e.ats.contains(&cur) => self.env51_sweep_at(cur, after),
            _ => 0,
        }
    }

    #[inline(never)]
    fn env51_sweep_at(&mut self, cur: *mut GcHeader, after: bool) -> i64 {
        let Some(e) = &mut self.env51_box else {
            return 0;
        };
        let (w, mut n, mut freed) = (self.current_white, 0, 0);
        let mut found = false;
        let mut first = usize::MAX;
        for g in e.gens.iter_mut() {
            let Some(a) = g.anchors.iter().find(|a| a.at == cur && a.before != after) else {
                continue;
            };
            found = true;
            first = first.min(a.recs[0]);
            for r in a.recs {
                let p = &mut g.ph[r];
                if p.dead {
                    continue;
                }
                n += 1;
                let f = p.hdr.flags;
                if is_white(f) && f & w == 0 {
                    p.dead = true;
                    p.hdr.flags = BLACK;
                    freed += p.size as usize;
                } else {
                    p.hdr.flags = w;
                }
            }
        }
        if !found {
            return 0;
        }
        if freed > 0 {
            self.acct_free(freed);
        }
        #[cfg(feature = "gc-trace")]
        self.trace.line(format_args!("e51 {n} {freed} {first}"));
        n
    }

    /// The object `h` is freed: no record is found by it any more.
    #[inline]
    pub(in crate::runtime) fn env51_forget(&mut self, h: *mut GcHeader) {
        if let Some(e) = &mut self.env51_box {
            if !e.owners.is_empty() {
                e.owners.retain(|o| o.0 != h);
            }
            if !e.ats.contains(&h) {
                return;
            }
            for g in e.gens.iter_mut() {
                for a in g.anchors.iter_mut().filter(|a| a.at == h) {
                    a.at = ptr::null_mut();
                }
            }
            e.ats.retain(|&a| a != h);
        }
    }

    /// The object `cur` is freed by a sweep: the records it ends swept
    /// after it, and none found by it any more.
    #[inline]
    pub(in crate::runtime) fn env51_freed(&mut self, cur: *mut GcHeader) {
        self.env51_sweep(cur, true);
        self.env51_forget(cur);
    }

    /// A cycle starts: the records are white, as PUC's objects are.
    pub(in crate::runtime) fn env51_whiten(&mut self) {
        let w = self.current_white;
        if let Some(e) = &mut self.env51_box {
            for g in e.gens.iter_mut() {
                for p in g.ph.iter_mut().filter(|p| !p.dead) {
                    p.hdr.flags = w;
                }
            }
        }
    }
}
