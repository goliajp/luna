//! What a marker borrows from the model while it marks: moved in when it is
//! lent out, moved back when it returns (`Heap::lend_model` and
//! `Heap::settle_model`, beside the loads' fields), so it reads the
//! records, the loads in progress without pointing into the model.

use crate::runtime::puc::records::env51::Env51;
use crate::runtime::puc::records::parse::ParseFrame;

/// The model's values a marker holds.
#[derive(Default)]
pub(in crate::runtime) struct Lent {
    /// 5.1's environment records
    pub(in crate::runtime) env51: Option<Box<Env51>>,
    /// the loads a step runs inside
    pub(in crate::runtime) parse: Vec<ParseFrame>,
}
