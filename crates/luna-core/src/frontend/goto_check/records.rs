//! The records of the goto check, and making or reusing a checker.

use super::*;
use crate::runtime::DebugName;

pub(super) struct Label {
    pub(super) name: DebugName,
    pub(super) line: u32,
    /// active locals where the label stands
    pub(super) nactvar: usize,
}

pub(super) struct Goto {
    pub(super) name: DebugName,
    pub(super) line: u32,
    /// active locals at the goto; lowered as it moves out of blocks
    pub(super) nactvar: usize,
}

pub(super) struct Block {
    pub(super) nactvar: usize,
    pub(super) first_label: usize,
    pub(super) first_goto: usize,
    pub(super) is_loop: bool,
}

/// A label whose trailing no-op statements are still being read: PUC
/// decides whether it ends its block (and so sits outside the block's
/// locals) only after skipping them.
pub(super) struct Open {
    pub(super) name: DebugName,
    pub(super) line: u32,
    /// 5.2/5.3 enter the label in the list before skipping
    pub(super) entry: Option<usize>,
}

impl GotoCheck {
    /// The checker for dialects with goto (5.2+), in the vectors of an
    /// earlier one when given.
    pub(crate) fn new(version: LuaVersion, old: Option<GotoCheck>) -> Option<GotoCheck> {
        if !version.has_goto() {
            return None;
        }
        let mut g = old.unwrap_or_else(|| GotoCheck {
            v54: false,
            v55: false,
            actvar: Vec::new(),
            names: String::new(),
            labels: Vec::new(),
            pending: Vec::new(),
            blocks: Vec::new(),
            funcs: Vec::new(),
            open: Vec::new(),
        });
        g.clear();
        g.v54 = version >= LuaVersion::Lua54;
        g.v55 = version >= LuaVersion::Lua55;
        Some(g)
    }

    /// Forget everything, keeping the vectors.
    fn clear(&mut self) {
        self.actvar.clear();
        self.names.clear();
        self.labels.clear();
        self.pending.clear();
        self.blocks.clear();
        self.funcs.clear();
        self.open.clear();
    }
}
