//! The strings of one load, each looked up in the heap's string table
//! once: the compiler and the naming of PUC's prototypes find them here by
//! the name's number, and the count's replay takes them from here too.

use super::*;

/// The names PUC's parser makes itself (`luaX_newliteral`), then `_ENV`.
pub(crate) const LITERALS: [&str; 10] = [
    "self",
    "arg",
    "(for index)",
    "(for limit)",
    "(for step)",
    "(for generator)",
    "(for state)",
    "(for control)",
    "(vararg table)",
    "_ENV",
];

/// How many of [`LITERALS`] PUC's parser makes as it declares a local.
pub(crate) const LOCAL_LITERALS: usize = 9;

/// Where the name of a local variable or an upvalue comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NameSrc {
    /// a name of the chunk
    Sym(ast::Sym),
    /// one of [`LITERALS`]
    Lit(u8),
}

impl NameSrc {
    /// The source of `name`, one of [`LITERALS`].
    pub(crate) fn literal(name: &str) -> NameSrc {
        let i = LITERALS
            .iter()
            .position(|&l| l == name)
            .expect("a literal name");
        NameSrc::Lit(i as u8)
    }
}

/// The heap strings of one load, by the chunk's name numbers and by
/// [`LITERALS`], with where the names of each prototype come from.
///
/// Every string here is held by the load's tree of prototypes, as a
/// constant or as one of PUC's names (`Proto::puc_names`): the count's
/// replay takes them across its collection steps, which keep that tree.
/// Code that takes a constant or a name away from a prototype must clear
/// its entry here at the same time.
pub(crate) struct LoadStrs {
    /// by name number: short strings only (any length in 5.1, which
    /// interns every string)
    pub(crate) syms: LVec<Option<Gc<LuaStr>>>,
    pub(crate) lits: [Option<Gc<LuaStr>>; LITERALS.len()],
    /// by function, in the order they open: where the sources of its names
    /// (its local variables', then its upvalues') start in `srcs`
    pub(crate) named: LVec<u32>,
    pub(crate) srcs: LVec<NameSrc>,
}

impl LoadStrs {
    /// Empty, nothing allocated, on `mem`.
    pub(crate) fn new(mem: crate::runtime::mem::MemRef) -> LoadStrs {
        LoadStrs {
            syms: LVec::new(mem),
            lits: [None; LITERALS.len()],
            named: LVec::new(mem),
            srcs: LVec::new(mem),
        }
    }

    /// Emptied for a chunk of `names` names.
    pub(super) fn reset(&mut self, names: usize) {
        self.syms.clear();
        self.syms.resize_or_abort(names, None);
        self.lits = [None; LITERALS.len()];
        self.named.clear();
        self.srcs.clear();
    }

    /// A function opens: its number in [`Self::named`].
    pub(super) fn open_fn(&mut self) -> u32 {
        let n = self.named.len() as u32;
        self.named.push_or_abort(0);
        n
    }

    /// Function `seq` closed with names from `locvars` then `upvals`.
    pub(super) fn close_fn(&mut self, seq: u32, locvars: &[NameSrc], upvals: &[NameSrc]) {
        self.named[seq as usize] = self.srcs.len() as u32;
        self.srcs.extend_from_slice_or_abort(locvars);
        self.srcs.extend_from_slice_or_abort(upvals);
    }

    /// The sources of the names of function `seq` (in opening order), `n`
    /// of them.
    pub(crate) fn names_of(&self, seq: usize, n: usize) -> &[NameSrc] {
        let start = self.named[seq] as usize;
        &self.srcs[start..start + n]
    }
}
