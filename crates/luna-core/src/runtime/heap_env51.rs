//! 5.1's function environments that luna keeps no object for.
//!
//! A C closure of PUC 5.1 has an environment, which traversing it marks
//! first: the globals for most library functions, the package table for
//! the loaders, `module` and `require`, and for the io library three
//! tables luna does not have (`newfenv`): E1, the environment of its
//! functions and of the files they open, holding the default input and
//! output and the closure `io_fclose`; E2, of the standard files, holding
//! `io_noclose`; E3, of `popen` and its files, holding `io_pclose`. A
//! userdata's environment is marked as it is. Here those tables and
//! closures are records: no objects, only a header to be marked by, PUC's
//! size as their work, and what each marks when traversed, in PUC's
//! order. A native's or a userdata's environment is in the class bits of
//! its header.

use super::*;

/// The class bits naming a 5.1 environment (`ENV_*`); none, the globals.
const ENV_SHIFT: u8 = 5;
pub(crate) const ENV_PACKAGE: u8 = 1;
pub(crate) const ENV_IO: u8 = 2;
pub(crate) const ENV_IO_STD: u8 = 3;
pub(crate) const ENV_IO_POPEN: u8 = 4;

/// In a phantom's `sub` byte, telling it from a ghost record.
const PHANTOM: u8 = 0x40;

// the records, in `Env51::ph`
const E1: usize = 0;
const E2: usize = 1;
const E3: usize = 2;
const FCLOSE: usize = 3;
const NOCLOSE: usize = 4;
const PCLOSE: usize = 5;
const NONE: u8 = u8::MAX;

/// A table or closure of PUC's luna has no object for.
#[repr(C)]
pub(crate) struct Phantom {
    hdr: GcHeader,
    /// PUC's size, the work of traversing it
    size: u32,
    /// the records it marks, in PUC's order
    kids: [u8; 1],
    /// the environment it marks first (a closure's)
    env: u8,
    /// freed by a sweep, as PUC frees it: marked no more
    dead: bool,
}

pub(crate) struct Env51 {
    ph: [Phantom; 6],
    /// the package table, the loaders' environment
    package: *mut GcHeader,
    /// E1's array: the default input and output
    io: [*mut GcHeader; 2],
    /// where the records are on PUC's list of objects, by the objects of
    /// luna's next to them: the records two of them are swept with
    /// reaching the object (`before`) or after it
    anchors: [Anchor; 3],
}

#[derive(Clone, Copy)]
struct Anchor {
    at: *mut GcHeader,
    before: bool,
    recs: [usize; 2],
}

impl Heap {
    /// Make 5.1's environment records, once.
    pub(crate) fn env51_records(&mut self) -> &mut Env51 {
        let s = self.sizes;
        let w = self.current_white;
        let rec = |size: usize, kid: usize, env: u8| {
            let mut hdr = GcHeader::new(ObjTag::Ghost);
            hdr.sub = PHANTOM;
            hdr.flags = w;
            Phantom {
                hdr,
                size: size as u32,
                kids: [if kid == usize::MAX { NONE } else { kid as u8 }],
                env,
                dead: false,
            }
        };
        let anchor = |recs| Anchor {
            at: ptr::null_mut(),
            before: true,
            recs,
        };
        let none = usize::MAX;
        self.env51_box.get_or_insert_with(|| {
            Box::new(Env51 {
                ph: [
                    rec(s.table + 2 * s.tvalue + s.node, FCLOSE, 0),
                    rec(s.table + s.node, NOCLOSE, 0),
                    rec(s.table + s.node, PCLOSE, 0),
                    rec(s.c_closure(0), none, 0),
                    rec(s.c_closure(0), none, ENV_IO),
                    rec(s.c_closure(0), none, ENV_IO),
                ],
                package: ptr::null_mut(),
                io: [ptr::null_mut(); 2],
                anchors: [
                    anchor([E1, FCLOSE]),
                    anchor([E2, NOCLOSE]),
                    anchor([E3, PCLOSE]),
                ],
            })
        })
    }

    /// The package table, the loaders' environment.
    pub(crate) fn env51_package(&mut self, t: Gc<Table>) {
        self.env51_records().package = t.as_ptr() as *mut GcHeader;
    }

    /// The default input and output, E1's array.
    pub(crate) fn env51_io(&mut self, input: Option<Gc<Userdata>>, output: Option<Gc<Userdata>>) {
        let h =
            |u: Option<Gc<Userdata>>| u.map_or(ptr::null_mut(), |u| u.as_ptr() as *mut GcHeader);
        self.env51_records().io = [h(input), h(output)];
    }

    /// Where the records are on PUC's list: E1 and `io_fclose` after the
    /// io library's first function, E2 and `io_noclose` before its last,
    /// E3 and `io_pclose` before `stderr`.
    pub(crate) fn env51_anchors(
        &mut self,
        first: *mut GcHeader,
        last: *mut GcHeader,
        stderr: *mut GcHeader,
    ) {
        let a = &mut self.env51_records().anchors;
        (a[0].at, a[0].before) = (first, false);
        (a[1].at, a[1].before) = (last, true);
        (a[2].at, a[2].before) = (stderr, true);
    }

    /// The sweep reaches `cur` (`after`: has swept it): the records swept
    /// with it, as PUC sweeps them. Returns how many.
    #[inline]
    pub(super) fn env51_sweep(&mut self, cur: *mut GcHeader, after: bool) -> i64 {
        let Some(e) = &mut self.env51_box else {
            return 0;
        };
        let Some(a) = e.anchors.iter().find(|a| a.at == cur && a.before != after) else {
            return 0;
        };
        let (w, mut n, mut freed) = (self.current_white, 0, 0);
        for r in a.recs {
            let p = &mut e.ph[r];
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
        if freed > 0 {
            self.acct_free(freed);
        }
        #[cfg(feature = "gc-trace")]
        self.trace.line(format_args!("e51 {n} {freed}"));
        n
    }

    /// The object `h` is freed: no record is found by it any more.
    #[inline]
    pub(super) fn env51_forget(&mut self, h: *mut GcHeader) {
        if let Some(e) = &mut self.env51_box {
            for a in e.anchors.iter_mut().filter(|a| a.at == h) {
                a.at = ptr::null_mut();
            }
        }
    }

    /// The object behind `h`, a native or a userdata, has environment `env`.
    pub(crate) fn set_env51(&mut self, h: *mut GcHeader, env: u8) {
        // SAFETY: `h` heads a live object; only its class byte is written
        unsafe { (*h).cls = ((*h).cls & !(7 << ENV_SHIFT)) | (env << ENV_SHIFT) };
    }

    /// A cycle starts: the records are white, as PUC's objects are.
    pub(super) fn env51_whiten(&mut self) {
        let w = self.current_white;
        if let Some(e) = &mut self.env51_box {
            for p in e.ph.iter_mut().filter(|p| !p.dead) {
                p.hdr.flags = w;
            }
        }
    }

    /// The pointer a marker reaches the records through.
    pub(super) fn env51_ptr(&self) -> *const Env51 {
        self.env51_box
            .as_deref()
            .map_or(ptr::null(), |e| e as *const Env51)
    }
}

/// Whether `h` is one of the records.
///
/// # Safety
/// `h` heads an allocated object or a record.
pub(super) unsafe fn is_phantom(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Ghost && (*h).sub == PHANTOM }
}

/// Whether the record `h` stands for a table (else a closure).
///
/// # Safety
/// `h` heads a record.
#[cfg(feature = "gc-trace")]
pub(super) unsafe fn phantom_is_table(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*(h as *const Phantom)).kids[0] != NONE }
}

/// The work of traversing the record `h`.
///
/// # Safety
/// `h` heads a record.
pub(super) unsafe fn phantom_size(h: *mut GcHeader) -> i64 {
    // SAFETY: the caller's contract; a record starts with its header
    unsafe { i64::from((*(h as *const Phantom)).size) }
}

impl Marker {
    /// Mark the environment the class bits `cls` name.
    pub(crate) fn mark_env51(&mut self, cls: u8) {
        let env = cls >> ENV_SHIFT;
        if env == 0 || self.env51.is_null() {
            return;
        }
        // SAFETY: the records live as long as the heap the marker marks
        let e = unsafe { &*self.env51 };
        let h = match env {
            ENV_PACKAGE => e.package,
            ENV_IO => &e.ph[E1].hdr as *const GcHeader as *mut GcHeader,
            ENV_IO_STD => &e.ph[E2].hdr as *const GcHeader as *mut GcHeader,
            _ => &e.ph[E3].hdr as *const GcHeader as *mut GcHeader,
        };
        if !h.is_null() {
            // SAFETY: a record, or the live package table
            unsafe { self.header(h) };
        }
    }

    /// Traverse the record `h`, as PUC traverses its table or closure:
    /// E1's array from its end, the default output then input, then its
    /// one node's value; a closure's environment.
    ///
    /// # Safety
    /// `h` heads a record of the marker's.
    pub(super) unsafe fn trace_phantom(&mut self, h: *mut GcHeader) {
        // SAFETY: the caller's contract; the records outlive the marking
        unsafe {
            let p = &*(h as *const Phantom);
            let e = &*self.env51;
            if std::ptr::eq(p, &e.ph[E1]) {
                for f in [e.io[1], e.io[0]] {
                    if !f.is_null() {
                        self.header(f);
                    }
                }
            }
            if p.env != 0 {
                self.mark_env51(p.env << ENV_SHIFT);
            }
            for &k in &p.kids {
                if k != NONE {
                    self.header(&e.ph[usize::from(k)].hdr as *const GcHeader as *mut GcHeader);
                }
            }
        }
    }
}

impl Heap {
    /// `R38_PROP`: how many objects PUC's list holds as the sweep finds it.
    #[cfg(feature = "gc-trace")]
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
            n += e.ph.iter().filter(|p| !p.dead).count() as i64;
        }
        self.trace.line(format_args!("n {n} {by:?}"));
    }
}

impl Heap {
    /// `R38_TRACE`: a thread of `stack` slots shrunk with `inuse` in use.
    #[cfg(feature = "gc-trace")]
    pub(crate) fn trace_shrink(&mut self, stack: u32, inuse: u32) {
        self.trace.line(format_args!("sh {stack} {inuse}"));
    }
}
