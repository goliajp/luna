//! What luna's tests set in a Vm's run words to take compiled code down
//! paths a test cannot steer a program onto.

use crate::vm::Vm;

/// Holds (`on`) or drops the run words' note that hot counting pauses,
/// with no pause begun or ended: the optimizing tier's code then leaves
/// each link it comes to to the Vm, as it does during a pause. A real
/// pause begins at a collection exit and ends when a trace is entered.
pub fn hold_run_words_quiet(vm: &Vm, on: bool) {
    vm.jit.words.quiet.set(u32::from(on));
}
