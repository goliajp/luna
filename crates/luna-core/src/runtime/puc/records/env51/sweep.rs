//! Sweeping 5.1's environment records where PUC's list has them.

use super::*;

impl Heap {
    /// The sweep reaches `cur` (`after`: has swept it): the records swept
    /// with it, as PUC sweeps them. Returns how many.
    ///
    /// # Safety
    /// `cur` heads an object of this heap that is still allocated.
    #[inline(always)]
    pub(in crate::runtime) unsafe fn env51_sweep(
        &mut self,
        cur: *mut GcHeader,
        after: bool,
    ) -> i64 {
        // an anchor is one of the io library's objects, with its environment
        // in the class bits: the others are passed by without a search
        // SAFETY: the caller's contract
        if unsafe { (*cur).cls } >> ENV_SHIFT == 0 {
            return 0;
        }
        match &self.puc.env51_box {
            Some(e) if e.ats.contains(&cur) => self.env51_sweep_at(cur, after),
            _ => 0,
        }
    }

    #[inline(never)]
    fn env51_sweep_at(&mut self, cur: *mut GcHeader, after: bool) -> i64 {
        let w = self.white();
        let (mut n, mut freed, mut found, mut first) = (0, 0, false, usize::MAX);
        let gens = self.puc.env51_box.as_ref().map_or(0, |e| e.gens.len());
        for i in 0..gens {
            // the records of this opening the sweep frees, each on its own
            // as PUC frees them (an anchor names `newfenv`'s closure before
            // its table, which it made first)
            // a group's records, each anchor's: two groups at most share one
            let mut each = [None; 4];
            let mut m = 0;
            let Some(e) = &mut self.puc.env51_box else {
                return 0;
            };
            let g = &mut e.gens[i];
            // two groups at one place: the newer (later anchor) first, as
            // the list has it
            for j in (0..g.anchors.len()).rev() {
                let a = g.anchors[j];
                if a.at != cur || a.before == after {
                    continue;
                }
                found = true;
                first = first.min(a.recs[1]);
                for (k, r) in a.recs.into_iter().enumerate() {
                    let p = &mut g.ph[r];
                    if p.dead {
                        continue;
                    }
                    n += 1;
                    let f = p.hdr.flags();
                    if is_white(f) && f & w == 0 {
                        p.dead = true;
                        p.hdr.set_flags(BLACK);
                        each[m + k] = Some((r, p.counted));
                    } else {
                        p.hdr.set_flags(w);
                    }
                }
                m += 2;
            }
            for (r, counted) in each.into_iter().flatten() {
                freed += self.env51_free_record(r, counted);
            }
        }
        if !found {
            return 0;
        }
        self.trace_line(format_args!("e51 {n} {freed} {first}"));
        n
    }

    /// Record `r` freed, its blocks one by one as PUC frees them (a
    /// table's: `luaH_free` frees the node, then the array, then the
    /// table), counted if it was when made. Returns the bytes.
    fn env51_free_record(&mut self, r: usize, counted: bool) -> usize {
        let s = self.puc.sizes;
        let parts: &[usize] = match r {
            crate::runtime::puc::records::env51::E1 => &[s.node, 2 * s.tvalue, s.table],
            crate::runtime::puc::records::env51::E2 | crate::runtime::puc::records::env51::E3 => {
                &[s.node, s.table]
            }
            _ => &[s.c_closure(0)],
        };
        for &b in parts {
            self.acct_part(b * usize::from(counted), 0);
        }
        parts.iter().sum()
    }

    /// The object `h` is freed: no record is found by it any more.
    #[inline]
    pub(in crate::runtime) fn env51_forget(&mut self, h: *mut GcHeader) {
        if let Some(e) = &mut self.puc.env51_box {
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
    ///
    /// # Safety
    /// `cur` heads an object of this heap that is still allocated.
    #[inline]
    pub(in crate::runtime) unsafe fn env51_freed(&mut self, cur: *mut GcHeader) {
        // SAFETY: the caller's contract
        unsafe { self.env51_sweep(cur, true) };
        self.env51_forget(cur);
    }

    /// A cycle starts: the records are white, as PUC's objects are.
    pub(in crate::runtime) fn env51_whiten(&mut self) {
        let w = self.white();
        if let Some(e) = &mut self.puc.env51_box {
            for g in e.gens.iter_mut() {
                for p in g.ph.iter_mut().filter(|p| !p.dead) {
                    p.hdr.set_flags(w);
                }
            }
        }
    }
}
