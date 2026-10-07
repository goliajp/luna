//! PUC 5.1's parse-time upvalue count: each nested function's declared
//! locals and the upvalues it accumulates, so that the 60-upvalue limit
//! trips while the parser is inside the offending function.

use super::*;
use crate::runtime::mem::{LMap, LVec, MemRef};

pub(super) struct FnUvSlot {
    pub(super) locals: LVec<Sym>,
    pub(super) upvalues: LMap<Sym, ()>,
    pub(super) line_defined: u32,
}

impl FnUvSlot {
    pub(super) fn new(mem: MemRef, line_defined: u32) -> FnUvSlot {
        FnUvSlot {
            locals: LVec::new(mem),
            upvalues: LMap::new(mem),
            line_defined,
        }
    }
}

impl Parser<'_> {
    pub(super) fn track_uv_51(&self) -> bool {
        !self.upval_chain_51.is_empty()
    }

    pub(super) fn add_local_51(&mut self, name: Sym) {
        if self.track_uv_51() {
            self.upval_chain_51
                .last_mut()
                .expect("fn ctx")
                .locals
                .push_or_abort(name);
        }
    }

    pub(super) fn snap_locals_51(&self) -> usize {
        if self.track_uv_51() {
            self.upval_chain_51.last().expect("fn ctx").locals.len()
        } else {
            0
        }
    }

    pub(super) fn restore_locals_51(&mut self, snap: usize) {
        if self.track_uv_51() {
            self.upval_chain_51
                .last_mut()
                .expect("fn ctx")
                .locals
                .truncate(snap);
        }
    }

    pub(super) fn enter_fn_51(&mut self, line_defined: u32) {
        if self.track_uv_51() {
            let mem = self.upval_chain_51.mem();
            self.upval_chain_51
                .push_or_abort(FnUvSlot::new(mem, line_defined));
        }
    }

    pub(super) fn leave_fn_51(&mut self) {
        if self.track_uv_51() {
            self.upval_chain_51.pop();
        }
    }

    /// PUC 5.1 `singlevaraux`-equivalent: resolve `name` against the current
    /// nested-function stack of declared locals, accumulating an upvalue entry
    /// in every intermediate function between the referencing site and the
    /// owning scope. Returns PUC 5.1's "has more than 60 upvalues" error the
    /// moment a link's upvalue set crosses 60. No-op for non-5.1 dialects.
    pub(super) fn ident_lookup_51(&mut self, name: Sym) -> Result<(), SyntaxError> {
        if !self.track_uv_51() {
            return Ok(());
        }
        const MAXUPVAL: usize = 60;
        let n = self.upval_chain_51.len();
        let mut owner: Option<usize> = None;
        for k in (0..n).rev() {
            if self.upval_chain_51[k].locals.contains(&name) {
                owner = Some(k);
                break;
            }
        }
        let Some(owner_idx) = owner else {
            return Ok(());
        };
        if owner_idx + 1 == n {
            return Ok(());
        }
        for k in (owner_idx + 1)..n {
            let inserted = self.upval_chain_51[k]
                .upvalues
                .insert_or_abort(name, ())
                .is_none();
            if inserted && self.upval_chain_51[k].upvalues.len() > MAXUPVAL {
                let line_defined = self.upval_chain_51[k].line_defined;
                let where_ = if k == 0 {
                    "main function".to_string()
                } else {
                    format!("function at line {line_defined}")
                };
                // 5.1 `errorlimit`: "<where> has more than <limit> <what>"
                return Err(SyntaxError {
                    line: self.tok.line,
                    msg: format!("{where_} has more than {MAXUPVAL} upvalues").into_bytes(),
                });
            }
        }
        Ok(())
    }
}
