//! What a marker borrows from the model while it marks: moved in when it is
//! lent out, moved back when it returns, so it reads the records, the
//! loads in progress and the stand-ins without pointing into the model.

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
    /// 5.2 and 5.3: the objects of PUC's luna's stand for, when the marking
    /// counts them
    pub(in crate::runtime) stands: Vec<(*mut GcHeader, usize)>,
    /// `stands` was taken (not left to the sizer, or to a full collection)
    stands_taken: bool,
}

impl Heap {
    /// What a marker borrows, `stands` with the stand-ins.
    pub(in crate::runtime) fn lend_model(&mut self, stands: bool) -> Lent {
        Lent {
            env51: self.puc.env51_box.take(),
            parse: std::mem::take(&mut self.puc.parse),
            stands: if stands {
                std::mem::take(&mut self.puc.stand_ins)
            } else {
                Vec::new()
            },
            stands_taken: stands,
        }
    }

    /// What a marker borrowed, back.
    pub(in crate::runtime) fn settle_model(&mut self, l: &mut Lent) {
        if let Some(e) = l.env51.take() {
            self.puc.env51_box = Some(e);
        }
        if !l.parse.is_empty() || self.puc.parse.is_empty() {
            self.puc.parse = std::mem::take(&mut l.parse);
        }
        if l.stands_taken {
            self.puc.stand_ins = std::mem::take(&mut l.stands);
            l.stands_taken = false;
        }
    }
}
