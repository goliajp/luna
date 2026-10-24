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

mod mark;
mod sweep;
#[cfg(feature = "gc-trace")]
mod trace;
#[cfg(feature = "gc-trace")]
pub(in crate::runtime) use trace::phantom_is_table;

/// The class bits naming a 5.1 environment (`ENV_*`); none, the globals.
const ENV_SHIFT: u8 = 5;
pub(crate) const ENV_PACKAGE: u8 = 1;
pub(crate) const ENV_IO: u8 = 2;
pub(crate) const ENV_IO_STD: u8 = 3;
pub(crate) const ENV_IO_POPEN: u8 = 4;
/// Not an environment: a userdata standing for a 5.1 prototype on the
/// stack, whose type is "proto" (see `Userdata::is_proto`).
pub(crate) const ENV_PROTO: u8 = 5;

/// In a phantom's `sub` byte, telling it from a ghost record.
const PHANTOM: u8 = 0x40;

// the records, in `Env51::ph`
pub(super) const E1: usize = 0;
pub(super) const E2: usize = 1;
pub(super) const E3: usize = 2;
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
    /// made while counting: its free is counted
    counted: bool,
}

/// The records of one opening of the io library: a second
/// `luaL_openlibs` makes its tables and closures anew, the old ones
/// living on as long as what holds them.
pub(crate) struct Gen {
    ph: [Phantom; 6],
    /// E1's array: the default input and output
    io: [*mut GcHeader; 2],
    /// where the records are on PUC's list of objects, by the objects of
    /// luna's next to them: the records two of them are swept with
    /// reaching the object (`before`) or after it
    anchors: [Anchor; 3],
    /// E1 was made: the next one belongs to a new opening
    opened: bool,
}

pub(crate) struct Env51 {
    // boxed: the records' headers live in a `Gen`, and the collector holds
    // pointers to them, so a `Gen` must not move when the list grows
    #[allow(clippy::vec_box)]
    gens: Vec<Box<Gen>>,
    /// the package table, the loaders' environment
    package: *mut GcHeader,
    /// the objects whose io environment is a later opening's, by the
    /// opening (the last entry of an object counts); the others' is the
    /// first's
    owners: Vec<(*mut GcHeader, u8)>,
    /// every anchor's object, for a sweep to pass the others by quickly
    ats: Vec<*mut GcHeader>,
    /// the key of the io library's tables' one node, "__close"
    close: *mut GcHeader,
}

impl Env51 {
    /// The opening the io environment of the object behind `h` is from.
    fn gen_of(&self, h: *mut GcHeader) -> &Gen {
        let i = self
            .owners
            .iter()
            .rev()
            .find(|o| o.0 == h)
            .map_or(0, |o| o.1);
        &self.gens[usize::from(i)]
    }
}

#[derive(Clone, Copy)]
struct Anchor {
    at: *mut GcHeader,
    before: bool,
    recs: [usize; 2],
}

impl Heap {
    /// The records of a new opening of the io library.
    fn env51_gen(&self) -> Box<Gen> {
        let s = self.puc.sizes;
        let w = self.white();
        let rec = |size: usize, kid: usize, env: u8| {
            let mut hdr = GcHeader::new(ObjTag::Ghost);
            hdr.sub = PHANTOM;
            hdr.set_flags(w);
            Phantom {
                hdr,
                size: size as u32,
                kids: [if kid == usize::MAX { NONE } else { kid as u8 }],
                env,
                dead: false,
                counted: false,
            }
        };
        let anchor = |recs| Anchor {
            at: ptr::null_mut(),
            before: true,
            recs,
        };
        let none = usize::MAX;
        Box::new(Gen {
            ph: [
                rec(s.table + 2 * s.tvalue + s.node, FCLOSE, 0),
                rec(s.table + s.node, NOCLOSE, 0),
                rec(s.table + s.node, PCLOSE, 0),
                rec(s.c_closure(0), none, 0),
                rec(s.c_closure(0), none, ENV_IO),
                rec(s.c_closure(0), none, ENV_IO),
            ],
            io: [ptr::null_mut(); 2],
            anchors: [
                anchor([FCLOSE, E1]),
                anchor([NOCLOSE, E2]),
                anchor([PCLOSE, E3]),
            ],
            opened: false,
        })
    }

    /// 5.1's environment records, made the first time.
    fn env51_records(&mut self) -> &mut Env51 {
        if self.puc.env51_box.is_none() {
            let g = self.env51_gen();
            self.puc.env51_box = Some(Box::new(Env51 {
                gens: vec![g],
                package: ptr::null_mut(),
                owners: Vec::new(),
                ats: Vec::new(),
                close: ptr::null_mut(),
            }));
        }
        self.puc.env51_box.as_deref_mut().expect("made above")
    }

    /// The records of the latest opening.
    fn env51_gen_now(&mut self) -> &mut Gen {
        let e = self.env51_records();
        e.gens.last_mut().expect("one at least")
    }

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
        let p = &mut g.ph[k];
        p.hdr.set_flags(w);
        (p.dead, p.counted) = (false, counted);
        &mut p.hdr as *mut GcHeader
    }

    /// Which record `h` heads, if one does.
    pub(crate) fn env51_record_index(&self, h: *mut GcHeader) -> Option<usize> {
        let e = self.puc.env51_box.as_deref()?;
        e.gens
            .iter()
            .find_map(|g| g.ph.iter().position(|p| std::ptr::eq(&p.hdr, h)))
    }

    /// The package table, the loaders' environment.
    pub(crate) fn env51_package(&mut self, t: Gc<Table>) {
        self.env51_records().package = t.as_ptr() as *mut GcHeader;
    }

    /// The default input (0) or output (1) in the latest E1's array.
    pub(crate) fn env51_io(&self, i: usize) -> Option<Gc<Userdata>> {
        let e = self.puc.env51_box.as_deref()?;
        let h = e.gens.last()?.io[i];
        // SAFETY: E1 holds its files: marking it marks them
        (!h.is_null()).then(|| unsafe { Gc::from_ptr(h as *mut Userdata) })
    }

    /// The latest E1's array entry `i` set to `u`, with the barrier of
    /// `lua_rawseti` into E1.
    pub(crate) fn env51_set_io(&mut self, i: usize, u: Gc<Userdata>) {
        let g = self.env51_gen_now();
        g.io[i] = u.as_ptr() as *mut GcHeader;
        let e1 = &mut g.ph[E1].hdr as *mut GcHeader;
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
        if let Some(e) = &mut self.puc.env51_box
            && e.gens.len() > 1
        {
            e.owners.retain(|o| o.0 != h);
            if matches!(env, ENV_IO | ENV_IO_STD | ENV_IO_POPEN) {
                e.owners.push((h, (e.gens.len() - 1) as u8));
            }
        }
    }
}

/// Whether `h` is one of the records.
///
/// # Safety
/// `h` heads an allocated object or a record.
pub(in crate::runtime) unsafe fn is_phantom(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Ghost && (*h).sub == PHANTOM }
}

/// The work of traversing the record `h`.
///
/// # Safety
/// `h` heads a record.
pub(in crate::runtime) unsafe fn phantom_size(h: *mut GcHeader) -> i64 {
    // SAFETY: the caller's contract; a record starts with its header
    unsafe { i64::from((*(h as *const Phantom)).size) }
}
