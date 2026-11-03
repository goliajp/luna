//! The buffers of a finished [`Lir`], kept for the next trace, and the
//! parts of one the optimizing tier keeps.

use super::*;

thread_local! {
    static SPARE: std::cell::RefCell<Option<Lir>> = const { std::cell::RefCell::new(None) };
}

impl Lir {
    /// A cleared `Lir`, reusing the buffers of the last one given back on
    /// this thread.
    pub(crate) fn take() -> Lir {
        match SPARE.with(|s| s.borrow_mut().take()) {
            Some(mut l) => {
                l.insts.clear();
                l.blocks.clear();
                l.value_ty.clear();
                l.konst.clear();
                l.relocs.clear();
                l.var_ty.clear();
                l.args.clear();
                l.slots.clear();
                let (nf, np) = l.helpers.map_or((0, 0), |(_, f, p)| (f, p));
                l.funcs.truncate(nf as usize);
                l.sigs.clear();
                l.param_tys.truncate(np as usize);
                l.snaps.clear();
                l.bparams.clear();
                l.bparam_vals.clear();
                l.var_cur.clear();
                l.arg0 = NONE;
                l.cur = NONE;
                l.unsupported = None;
                l.loop_entry = None;
                l.pending_inherit = None;
                l
            }
            None => Lir::new(),
        }
    }

    pub(crate) fn give(self) {
        SPARE.with(|s| *s.borrow_mut() = Some(self));
    }
}

impl Lir {
    /// Bytes `self` takes, roughly.
    pub(crate) fn size(&self) -> usize {
        std::mem::size_of::<Lir>()
            + self.insts.len() * std::mem::size_of::<Inst>()
            + self.blocks.len() * std::mem::size_of::<BlockData>()
            + self.value_ty.len()
            + 4 * (self.args.len() + self.bparams.len())
    }

    /// The parts of `self` code generation reads, in buffers of their own
    /// (kept for the optimizing tier).
    pub(crate) fn detach(&self) -> Lir {
        Lir {
            insts: self.insts.clone(),
            blocks: self
                .blocks
                .iter()
                .map(|b| BlockData {
                    first: b.first,
                    last: b.last,
                    len: b.len,
                    params_at: b.params_at,
                    n_params: b.n_params,
                    ..BlockData::default()
                })
                .collect(),
            value_ty: self.value_ty.clone(),
            relocs: self.relocs.clone(),
            var_ty: self.var_ty.clone(),
            args: self.args.clone(),
            slots: self.slots.clone(),
            funcs: self.funcs.clone(),
            sigs: self.sigs.clone(),
            param_tys: self.param_tys.clone(),
            bparams: self.bparams.clone(),
            arg0: self.arg0,
            loop_entry: self.loop_entry.clone(),
            ..Lir::new()
        }
    }
}
