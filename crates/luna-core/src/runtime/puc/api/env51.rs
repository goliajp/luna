//! 5.1's records of the io library's environments: what the opening
//! makes and sets, and the barrier of a store between two records.

use super::super::*;
use crate::runtime::puc::records::env51::*;

impl Heap {
    /// Record `k` made now (the opening counted it): current white and
    /// live; its header, which names it on the stack. E1 made again
    /// starts the records of another opening.
    pub(crate) fn env51_born(&mut self, k: usize) -> *mut GcHeader {
        if k == E1 && self.env51_gen_now().opened {
            let g = self.env51_gen();
            self.env51_records().gens.push(g);
        }
        let w = self.white();
        let counted = self.counting();
        let g = self.env51_gen_now();
        g.opened |= k == E1;
        g.ph.with(k, |p| p.born(w, counted));
        g.ph.hdr(k)
    }

    /// Which record `h` heads, if one does.
    pub(crate) fn env51_record_index(&self, h: *mut GcHeader) -> Option<usize> {
        self.env51_record(h).map(|(k, _)| k)
    }

    /// The record `h` heads, if one does: its number and the records' own
    /// pointer to it, which `h` (maybe a light userdata's address) only
    /// names.
    fn env51_record(&self, h: *mut GcHeader) -> Option<(usize, *mut GcHeader)> {
        let e = self.puc.records.env51_box.as_deref()?;
        e.gens
            .iter()
            .find_map(|g| (0..6).map(|k| (k, g.ph.hdr(k))).find(|&(_, r)| r == h))
    }

    /// The package table, the loaders' environment.
    pub(crate) fn env51_package(&mut self, t: Gc<Table>) {
        self.env51_records().package = t.as_ptr() as *mut GcHeader;
    }

    /// The default input (0) or output (1) in the latest E1's array.
    pub(crate) fn env51_io(&self, i: usize) -> Option<Gc<Userdata>> {
        let e = self.puc.records.env51_box.as_deref()?;
        let h = e.gens.last()?.io[i];
        // SAFETY: E1 holds its files: marking it marks them
        (!h.is_null()).then(|| unsafe { Gc::from_ptr(h as *mut Userdata) })
    }

    /// The latest E1's array entry `i` set to `u`, with the barrier of
    /// `lua_rawseti` into E1.
    pub(crate) fn env51_set_io(&mut self, i: usize, u: Gc<Userdata>) {
        let g = self.env51_gen_now();
        g.io[i] = u.as_ptr() as *mut GcHeader;
        let e1 = g.ph.hdr(E1);
        // SAFETY: a record lives as long as the heap
        unsafe { self.barrier_back_obj(e1) };
    }

    /// Where the latest records are on PUC's list: E1 and `io_fclose`
    /// after the io library's first function, E2 and `io_noclose` before
    /// its last, E3 and `io_pclose` before them (the standard files they
    /// were made after are at the list's end).
    pub(crate) fn env51_anchors(&mut self, first: *mut GcHeader, last: *mut GcHeader) {
        let a = &mut self.env51_gen_now().anchors;
        (a[0].at, a[0].before) = (first, false);
        (a[1].at, a[1].before) = (last, true);
        (a[2].at, a[2].before) = (last, true);
        let e = self.env51_records();
        e.ats = e
            .gens
            .iter()
            .flat_map(|g| g.anchors.iter().map(|a| a.at))
            .collect();
        e.ats.retain(|h| !h.is_null());
    }

    /// The object behind `h`, a native or a userdata, has environment
    /// `env`: one of the io library's is the latest opening's.
    pub(crate) fn set_env51(&mut self, h: Gc<GcHeader>, env: u8) {
        let h = h.as_ptr();
        // SAFETY: `h` heads a live object; only its class and flag bytes
        // are written: one with an environment to mark is no leaf
        unsafe {
            (*h).cls = ((*h).cls & !(7 << ENV_SHIFT)) | (env << ENV_SHIFT);
            if env != 0 && env != ENV_PROTO {
                (*h).clear_leaf();
            }
        }
        if let Some(e) = &mut self.puc.records.env51_box
            && e.gens.len() > 1
        {
            e.owners.retain(|o| o.0 != h);
            if matches!(env, ENV_IO | ENV_IO_STD | ENV_IO_POPEN) {
                e.owners.push((h, (e.gens.len() - 1) as u8));
            }
        }
    }

    /// `lua_setfield` of `key` and the record `v` (a closure) into the
    /// record `t`, one of the io library's tables: 5.1's `luaC_barriert`.
    /// Both must be records of this heap's.
    pub(crate) fn env51_set_key(&mut self, t: *mut GcHeader, key: Gc<LuaStr>, v: *mut GcHeader) {
        let (Some((_, tr)), Some((_, vr))) = (self.env51_record(t), self.env51_record(v)) else {
            panic!("a store between records of the opening");
        };
        self.env51_records().close = key.as_ptr() as *mut GcHeader;
        // SAFETY: the records' own pointers (the arguments are only compared
        // with them), boxed in the records, which live as long as the heap;
        // only flags are read
        unsafe {
            if is_white((*vr).flags()) {
                self.barrier_back_obj(tr);
            }
        }
    }
}
