//! Closing a function's level into its prototype, and closing every level
//! a failed compile left open: a counted load that fails on a compile
//! error is replayed from the prototypes as PUC's code generator left
//! them, up to the token it failed on.

use super::*;
use crate::frontend::error::NearKind;

/// What a compile keeps for the replay of a load that failed.
#[derive(Default)]
pub(crate) struct ReplayOpts {
    /// an assignment to a read-only variable compiles (as to register 0)
    pub(crate) lenient: bool,
    /// a compile that fails closes the functions it has open (`sealed`)
    pub(crate) seal: bool,
    pub(crate) sealed: Option<Gc<Proto>>,
    /// the token the last compile error was placed at, and what kind of
    /// token its message shows (`None`: none)
    pub(crate) at: Option<(u32, NearKind)>,
}

impl Compiler<'_> {
    /// The innermost level's function on the heap; the level stays,
    /// emptied, for the next function at this depth.
    pub(super) fn close_level(&mut self, line: u32, last_line: u32) -> Gc<Proto> {
        self.depth -= 1;
        let lvl = &mut self.levels[self.depth];
        if self.version <= LuaVersion::Lua53 {
            for i in lvl.code.iter_mut() {
                *i = crate::vm::isa::imm_form::to_imm(*i, &lvl.consts);
            }
        }
        let proto = lvl.take_proto(self.source, line, last_line, self.heap);
        self.strs
            .close_fn(lvl.seq, &lvl.locvar_srcs, &lvl.upval_srcs);
        lvl.locvar_srcs.clear();
        lvl.upval_srcs.clear();
        self.heap.adopt_proto(proto)
    }

    /// Every open level closed as it stands, each in the one enclosing it
    /// as its last nested function: the main function's prototype.
    pub(super) fn seal_levels(&mut self) -> Gc<Proto> {
        while self.depth > 1 {
            let line = self.lr().line_defined;
            let p = self.close_level(line, 0);
            self.l().protos.push_or_abort(p);
        }
        self.close_level(0, 0)
    }
}
