//! 5.5's count of what a marking marks (`GCmarked`).

use super::*;

/// What a 5.5 marking needs to count the objects it marks (`GCmarked`):
/// the sizes, luna's own tables (lent by the heap meanwhile) and the
/// threads the VM holds.
pub(crate) struct Sizer {
    sizes: crate::runtime::puc_size::PucSizes,
    own: Vec<*const Table>,
    stand_ins: Vec<(*mut GcHeader, usize)>,
    run: Running,
    pub(in crate::runtime::heap) marked: i64,
}

impl Sizer {
    /// 5.5's `objsize` of the object behind `h`, which marking it adds.
    ///
    /// # Safety
    /// `h` heads an allocated object.
    pub(in crate::runtime::heap) unsafe fn size(&self, h: *mut GcHeader) -> i64 {
        let d = crate::runtime::table::Dialect::L55;
        // SAFETY: the caller's contract; the tag names the type
        unsafe {
            if (*h).tag != ObjTag::Coro {
                if is_uncounted(h) {
                    // what an object of luna's stands for
                    let s = self.stand_ins.iter().filter(|s| s.0 == h);
                    return s.map(|s| s.1).sum::<usize>() as i64;
                }
                return pace::objsize(&self.sizes, d, &self.own, h) as i64;
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
            pace::thread_bytes(&self.sizes, d, size, ci) as i64
        }
    }
}

impl Heap {
    /// A sizer for a marker, when markings count `GCmarked`.
    pub(in crate::runtime::heap) fn lend_sizer(&mut self) -> Option<Sizer> {
        let run = self.pace.run.filter(|_| self.pace.count_marked)?;
        Some(Sizer {
            sizes: self.sizes,
            own: std::mem::take(&mut self.puc.own_tables),
            stand_ins: std::mem::take(&mut self.puc.stand_ins),
            run,
            marked: 0,
        })
    }

    /// The sizer `m` had back: what it counted, and the tables it held.
    pub(in crate::runtime::heap) fn settle_sizer(&mut self, m: &mut Marker) {
        if let Some(s) = m.sizer.take() {
            self.pace.estimate += s.marked;
            self.puc.own_tables = s.own;
            self.puc.stand_ins = s.stand_ins;
        }
    }

    /// The object behind `h` marked outside a marker (a barrier): 5.5
    /// counts it.
    ///
    /// # Safety
    /// `h` heads an allocated object of this heap.
    pub(in crate::runtime::heap) unsafe fn count_marked(&mut self, h: *mut GcHeader) {
        if let Some(s) = self.lend_sizer() {
            // SAFETY: the caller's contract
            let n = unsafe { s.size(h) };
            self.pace.estimate += n;
            self.puc.own_tables = s.own;
            self.puc.stand_ins = s.stand_ins;
        }
    }
}
