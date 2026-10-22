//! Collection steps inside a load: PUC's parser checks for a collection
//! where it anchors a new string and where a function ends
//! (`luaC_checkGC`), its objects on the loading thread's stack, the
//! prototypes it is building holding what it has put in them so far.

use super::*;
use crate::runtime::heap::GcHeader;
use crate::runtime::heap::parse::{OpenView, ParseFrame, set_parsing};

/// The header of the object of `g`.
fn hdr<T: crate::runtime::heap::GcObject>(g: Gc<T>) -> *mut GcHeader {
    g.as_ptr() as *mut GcHeader
}

impl Vm {
    /// A collection check of PUC's parser, which holds `temp` on the stack
    /// meanwhile. A step that is due runs with the load's objects as PUC
    /// has them; an error it raises (a finalizer's, up to 5.3) is the
    /// load's.
    pub(super) fn load_checkpoint(
        &mut self,
        r: &mut Replay,
        temp: Option<Gc<crate::runtime::LuaStr>>,
    ) -> Result<(), SyntaxError> {
        if r.rec.is_some() {
            return Ok(());
        }
        if self.heap.puc.load_holds {
            #[cfg(feature = "gc-trace")]
            self.trace_checkpoint();
            return Ok(());
        }
        if self.checkpoint_idle() {
            return Ok(());
        }
        let due = self.heap.gc_due();
        let frame = self.parse_frame(r, temp);
        self.heap.puc.parse.push(frame);
        let stepped = self.native_checkpoint_due();
        self.heap.puc.parse.pop();
        // 5.1 and 5.2 drop a function's table when it ends: a name the step
        // freed is made again where it is scanned next
        if due && matches!(r.d, Dialect::L51 | Dialect::L52) {
            for s in &mut r.syms {
                s.0 = None;
            }
        }
        stepped.map_err(|e| self.load_failed_by(e))
    }

    /// The parser's objects on the stack (5.1 each function's table and
    /// prototype; 5.2 the main closure and each function's table; 5.3 and
    /// 5.4 the main closure and the scanner's table; 5.5 the anchor
    /// table), and what the prototypes being built hold, their arrays as
    /// grown so far.
    fn parse_frame(&self, r: &Replay, temp: Option<Gc<crate::runtime::LuaStr>>) -> ParseFrame {
        let cl = r.cl.map_or(std::ptr::null_mut(), hdr);
        let tab = |t: &Option<PTab>| t.as_ref().and_then(PTab::tab);
        let mut slots = Vec::with_capacity(2 * r.fs.len() + 3);
        match r.d {
            Dialect::L51 => {
                for f in &r.fs {
                    slots.extend(tab(&f.h));
                    slots.extend(r.opening.get(f.shape).map(|&p| hdr(p)));
                }
            }
            Dialect::L52 => {
                slots.push(cl);
                slots.extend(r.fs.iter().filter_map(|f| tab(&f.h)));
                slots.extend(r.closing_tab);
            }
            Dialect::L53 | Dialect::L54 => {
                slots.push(cl);
                slots.extend(tab(&r.h));
            }
            Dialect::L55 => slots.extend(tab(&r.h)),
        }
        slots.retain(|h| !h.is_null());
        slots.extend(temp.map(hdr));
        let s = self.heap.puc_sizes();
        let mut open = Vec::with_capacity(r.fs.len());
        for (i, f) in r.fs.iter().enumerate() {
            let Some(p) = r.opening.get(f.shape) else {
                continue;
            };
            p.puc_arrays.set(f.puc_caps(r.d, &s));
            let end = r.fs.get(i + 1).map_or(r.kstr.len(), |g| g.kbase);
            open.push(OpenView {
                proto: hdr(*p),
                k: (f.kbase, end),
                kids: f.p.n,
                ups: f.up.n,
                locs: f.loc.n,
            });
        }
        ParseFrame {
            slots,
            closure: cl,
            open,
            kstr: r.kstr.iter().map(|&s| hdr(s)).collect(),
        }
    }

    /// The replay is over, `done` or not: what it was building is built
    /// (or garbage), and what a step of this cycle marked of it holds what
    /// luna's own objects hold besides (a chunk loaded gets its upvalue
    /// after this, with its barrier).
    pub(super) fn replay_left(&mut self, r: &Replay, done: bool) {
        if r.rec.is_some() {
            return;
        }
        for f in &r.fs {
            if let Some(&p) = r.opening.get(f.shape) {
                set_parsing(p, false);
                self.heap.settle_parsed(p);
            }
        }
        if let Some(cl) = r.cl {
            set_parsing(cl, false);
            if !done {
                self.heap.settle_closure(cl);
            }
        }
    }

    /// PUC's `luaC_objbarrier` from the prototype of open function `i` to
    /// `child`, which the parser has just put in it.
    pub(super) fn proto_barrier(&mut self, r: &Replay, i: usize, child: *mut GcHeader) {
        if r.rec.is_some() {
            return;
        }
        let Some(&p) = r.fs.get(i).and_then(|f| r.opening.get(f.shape)) else {
            return;
        };
        // SAFETY: both are live objects of this heap
        unsafe { self.heap.parse_barrier(hdr(p), child) };
    }

    /// The barrier from open function `i`'s prototype to its `n`th upvalue
    /// name (`up`) or local variable name, just registered.
    pub(super) fn name_barrier(&mut self, r: &Replay, i: usize, n: usize, up: bool) {
        let Some(&p) = r.fs.get(i).and_then(|f| r.opening.get(f.shape)) else {
            return;
        };
        let at = if up { p.locvars.len() + n } else { n }.wrapping_sub(1);
        let name = p.puc_names.borrow().get(at).copied();
        if let Some(s) = name {
            self.proto_barrier(r, i, hdr(s));
        }
    }
}
