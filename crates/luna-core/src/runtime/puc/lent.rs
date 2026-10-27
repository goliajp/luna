//! What a marker borrows from the model while it marks: moved in when it is
//! lent out, moved back when it returns, so it reads the records, the
//! loads in progress without pointing into the model.

use super::*;
use crate::runtime::puc::records::env51::Env51;
use crate::runtime::puc::records::parse::ParseFrame;

/// The model's values a marker holds.
#[derive(Default)]
pub(crate) struct Lent {
    /// 5.1's environment records
    pub(in crate::runtime) env51: Option<Box<Env51>>,
    /// the loads a step runs inside
    pub(in crate::runtime) parse: Vec<ParseFrame>,
}

impl Heap {
    /// What a marker borrows.
    pub(in crate::runtime::puc) fn lend_model(&mut self) -> Lent {
        Lent {
            env51: self.puc.records.env51_box.take(),
            parse: std::mem::take(&mut self.puc.loads.parse),
        }
    }

    /// What a marker borrowed, back.
    pub(in crate::runtime::puc) fn settle_model(&mut self, l: &mut Lent) {
        if let Some(e) = l.env51.take() {
            self.puc.records.env51_box = Some(e);
        }
        if !l.parse.is_empty() || self.puc.loads.parse.is_empty() {
            self.puc.loads.parse = std::mem::take(&mut l.parse);
        }
    }
}
