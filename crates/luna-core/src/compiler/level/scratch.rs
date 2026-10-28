//! What the compiler keeps between loads.

use super::*;

/// What the compiler keeps between loads: the vectors of finished
/// functions ([`LevelBufs`]), one per nesting level reached so far.
pub(crate) struct CompileScratch {
    pub(in crate::compiler) levels: LVec<LevelBufs>,
    /// the stack of functions being compiled, empty
    pub(in crate::compiler) open: LVec<Level<'static>>,
    /// the last compile's strings (see [`super::super::LoadStrs`])
    pub(crate) strs: super::super::LoadStrs,
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
            strs: super::super::LoadStrs::new(mem),
            spine: LVec::new(mem),
            lvs: LVec::new(mem),
            operands: LVec::new(mem),
            err_tok: None,
            puc_log: LVec::new(mem),
            replay: Default::default(),
        }
    }

    /// The most instructions (and records of what PUC allocates) whose
    /// room is kept between loads: a huge chunk's vectors are given back
    /// rather than held on to.
    const MAX_CODE: usize = 1 << 14;

    /// Give back the vectors of a huge chunk's compile.
    pub(crate) fn release_huge(&mut self) {
        let huge = self.puc_log.capacity() > Self::MAX_CODE
            || self
                .levels
                .iter()
                .any(|b| b.code_capacity() > Self::MAX_CODE);
        if !huge {
            return;
        }
        let mem = self.puc_log.mem();
        self.levels = LVec::new(mem);
        self.open = LVec::new(mem);
        self.strs = super::super::LoadStrs::new(mem);
        self.spine = LVec::new(mem);
        self.lvs = LVec::new(mem);
        self.operands = LVec::new(mem);
        self.puc_log = LVec::new(mem);
    }

    /// The token PUC reports `e`, the error the last compile returned, at.
    pub(crate) fn error_token(&mut self, e: &SyntaxError) -> Option<(super::super::TokRef, bool)> {
        self.err_tok
            .take()
            .filter(|(_, m, _)| *m == e.msg)
            .map(|(t, _, near)| (t, near))
    }
}
