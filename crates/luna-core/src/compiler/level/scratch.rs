//! What the compiler keeps between loads.

use super::*;

/// What the compiler keeps between loads: the vectors of finished
/// functions ([`LevelBufs`]), one per nesting level reached so far.
pub(crate) struct CompileScratch {
    pub(in crate::compiler) levels: LVec<LevelBufs>,
    /// the stack of functions being compiled, empty
    pub(in crate::compiler) open: LVec<Level<'static>>,
    pub(in crate::compiler) sym_strs: LVec<Option<Gc<LuaStr>>>,
    pub(in crate::compiler) spine: LVec<super::super::expr::Pending>,
    pub(in crate::compiler) lvs: LVec<super::super::lvalue::Lv>,
    pub(in crate::compiler) operands: LVec<ExprId>,
    /// the token of the last error the compiler returned, and its message
    /// (see [`super::super::TokRef`])
    pub(crate) err_tok: Option<(super::super::TokRef, Vec<u8>, bool)>,
    /// the last compile's log of what PUC allocates (see [`super::super::puc_log`])
    pub(crate) puc_log: LVec<super::super::puc_rec::Rec>,
    /// see [`super::super::ReplayOpts`]
    pub(crate) replay: super::super::ReplayOpts,
}

impl CompileScratch {
    /// Empty vectors on `mem`, nothing allocated.
    pub(crate) fn new(mem: MemRef) -> CompileScratch {
        CompileScratch {
            levels: LVec::new(mem),
            open: LVec::new(mem),
            sym_strs: LVec::new(mem),
            spine: LVec::new(mem),
            lvs: LVec::new(mem),
            operands: LVec::new(mem),
            err_tok: None,
            puc_log: LVec::new(mem),
            replay: Default::default(),
        }
    }

    /// The token PUC reports `e`, the error the last compile returned, at.
    pub(crate) fn error_token(&mut self, e: &SyntaxError) -> Option<(super::super::TokRef, bool)> {
        self.err_tok
            .take()
            .filter(|(_, m, _)| *m == e.msg)
            .map(|(t, _, near)| (t, near))
    }
}
