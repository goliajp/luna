//! 5.5's count of what a marking marks (`GCmarked`).

use super::*;

/// What a 5.5 marking needs to count the objects it marks (`GCmarked`):
/// the sizes, the stand-ins and keepers (lent by the heap meanwhile) and
/// the threads the VM holds.
pub(crate) struct Sizer {
    sizes: crate::runtime::puc::size::PucSizes,
    stand_ins: Vec<(*mut GcHeader, usize)>,
    keepers: Vec<(*mut GcHeader, *mut GcHeader)>,
    run: Running,
    pub(in crate::runtime) marked: i64,
}

impl Sizer {
    /// 5.5's `objsize` of the object behind `h`, which marking it adds.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    pub(in crate::runtime) unsafe fn size(&self, h: *mut GcHeader) -> i64 {
        let d = crate::runtime::dialect::Dialect::L55;
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            if (*h).tag != ObjTag::Coro {
                // what an object stands for besides itself (all of it, for
                // one of luna's own): PUC marks those objects with it
                let uncounted = is_uncounted(h);
                // (every object with stand-ins is marked STANDS)
                let stands = if (*h).cls & STANDS != 0 {
                    let s = self.stand_ins.iter().filter(|s| s.0 == h);
                    (s.map(|s| s.1).sum::<usize>()
                        + crate::runtime::puc::records::keeper::kept_bytes(&self.keepers, h))
                        as i64
                } else {
                    0
                };
                if uncounted {
                    return stands;
                }
                return crate::runtime::puc::work::objsize(&self.sizes, d, h) as i64 + stands;
            }
            let r = &self.run;
            let (size, ci) = if h == r.h {
                (r.size, r.ci)
            } else if h == r.main {
                (r.main_size, r.main_ci)
            } else {
                let c = &*(h as *const crate::runtime::Coro);
                (c.stack_size, c.ci_alloc)
            };
            crate::runtime::puc::work::thread_bytes(&self.sizes, d, size, ci) as i64
        }
    }
}

impl Heap {
    /// A sizer for a marker, when markings count `GCmarked`.
    pub(in crate::runtime) fn lend_sizer(&mut self) -> Option<Sizer> {
        let run = self.puc.pace.run.filter(|_| self.puc.pace.count_marked)?;
        Some(Sizer {
            sizes: self.puc.sizes,
            stand_ins: std::mem::take(&mut self.puc.stand_ins),
            keepers: std::mem::take(&mut self.puc.keepers),
            run,
            marked: 0,
        })
    }

    /// The sizer `m` had back: what it counted, and what it held.
    pub(in crate::runtime) fn settle_sizer(&mut self, m: &mut Marker) {
        if let Some(s) = m.take_sizer() {
            self.puc.pace.estimate += s.marked;
            self.puc.stand_ins = s.stand_ins;
            self.puc.keepers = s.keepers;
        }
    }

    /// The object behind `h` marked outside a marker (a barrier): 5.5
    /// counts it.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(in crate::runtime) unsafe fn count_marked(&mut self, h: *mut GcHeader) {
        if let Some(s) = self.lend_sizer() {
            // SAFETY: the caller's contract
            let n = unsafe { s.size(h) };
            self.puc.pace.estimate += n;
            self.puc.stand_ins = s.stand_ins;
            self.puc.keepers = s.keepers;
        }
    }
}
