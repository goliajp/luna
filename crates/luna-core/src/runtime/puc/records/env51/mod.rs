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
mod phantoms;
mod sweep;
#[cfg(feature = "gc-trace")]
mod trace;
pub(in crate::runtime::puc) use phantoms::Phantoms;
#[cfg(feature = "gc-trace")]
pub(in crate::runtime::puc) use trace::phantom_is_table;

/// The class bits naming a 5.1 environment (`ENV_*`); none, the globals.
pub(in crate::runtime::puc) const ENV_SHIFT: u8 = 5;
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
pub(in crate::runtime::puc) const E1: usize = 0;
pub(super) const E2: usize = 1;
pub(super) const E3: usize = 2;
const FCLOSE: usize = 3;
const NOCLOSE: usize = 4;
const PCLOSE: usize = 5;
const NONE: u8 = u8::MAX;

/// A table or closure of PUC's luna has no object for.
#[repr(C)]
pub(in crate::runtime::puc) struct Phantom {
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

impl Phantom {
    /// Made now: `white` and live, its free counted when `counted`.
    pub(in crate::runtime::puc) fn born(&mut self, white: u8, counted: bool) {
        self.hdr.set_flags(white);
        (self.dead, self.counted) = (false, counted);
    }
}

/// The records of one opening of the io library: a second
/// `luaL_openlibs` makes its tables and closures anew, the old ones
/// living on as long as what holds them.
pub(in crate::runtime::puc) struct Gen {
    pub(in crate::runtime::puc) ph: Phantoms,
    /// E1's array: the default input and output
    pub(in crate::runtime::puc) io: [*mut GcHeader; 2],
    /// where the records are on PUC's list of objects, by the objects of
    /// luna's next to them: the records two of them are swept with
    /// reaching the object (`before`) or after it
    pub(in crate::runtime::puc) anchors: [Anchor; 3],
    /// E1 was made: the next one belongs to a new opening
    pub(in crate::runtime::puc) opened: bool,
}

pub(in crate::runtime) struct Env51 {
    // boxed: the records' headers live in a `Gen`, and the collector holds
    // pointers to them, so a `Gen` must not move when the list grows
    #[allow(clippy::vec_box)]
    pub(in crate::runtime::puc) gens: Vec<Box<Gen>>,
    /// the package table, the loaders' environment
    pub(in crate::runtime::puc) package: *mut GcHeader,
    /// the objects whose io environment is a later opening's, by the
    /// opening (the last entry of an object counts); the others' is the
    /// first's
    pub(in crate::runtime::puc) owners: Vec<(*mut GcHeader, u8)>,
    /// every anchor's object, for a sweep to pass the others by quickly
    pub(in crate::runtime::puc) ats: Vec<*mut GcHeader>,
    /// the key of the io library's tables' one node, "__close"
    pub(in crate::runtime::puc) close: *mut GcHeader,
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
pub(in crate::runtime::puc) struct Anchor {
    pub(in crate::runtime::puc) at: *mut GcHeader,
    pub(in crate::runtime::puc) before: bool,
    recs: [usize; 2],
}

impl Heap {
    /// The records of a new opening of the io library.
    pub(in crate::runtime::puc) fn env51_gen(&self) -> Box<Gen> {
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
            ph: Phantoms::new([
                rec(s.table + 2 * s.tvalue + s.node, FCLOSE, 0),
                rec(s.table + s.node, NOCLOSE, 0),
                rec(s.table + s.node, PCLOSE, 0),
                rec(s.c_closure(0), none, 0),
                rec(s.c_closure(0), none, ENV_IO),
                rec(s.c_closure(0), none, ENV_IO),
            ]),
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
    pub(in crate::runtime::puc) fn env51_records(&mut self) -> &mut Env51 {
        if self.puc.records.env51_box.is_none() {
            let g = self.env51_gen();
            self.puc.records.env51_box = Some(Box::new(Env51 {
                gens: vec![g],
                package: ptr::null_mut(),
                owners: Vec::new(),
                ats: Vec::new(),
                close: ptr::null_mut(),
            }));
        }
        self.puc
            .records
            .env51_box
            .as_deref_mut()
            .expect("made above")
    }

    /// The records of the latest opening.
    pub(in crate::runtime::puc) fn env51_gen_now(&mut self) -> &mut Gen {
        let e = self.env51_records();
        e.gens.last_mut().expect("one at least")
    }
}

/// Whether `h` is one of the records.
///
/// # Safety
/// `h` heads an allocated object or a record.
pub(in crate::runtime::puc) unsafe fn is_phantom(h: *mut GcHeader) -> bool {
    // SAFETY: the caller's contract
    unsafe { (*h).tag == ObjTag::Ghost && (*h).sub == PHANTOM }
}

/// The work of traversing the record `h`.
///
/// # Safety
/// `h` heads a record.
pub(in crate::runtime::puc) unsafe fn phantom_size(h: *mut GcHeader) -> i64 {
    // SAFETY: the caller's contract; a record starts with its header
    unsafe { i64::from((*(h as *const Phantom)).size) }
}
