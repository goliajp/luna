//! What luna's tests set in a Vm's run words to take compiled code down
//! paths a test cannot steer a program onto.

use crate::vm::Vm;

/// Holds (`on`) or drops the run words' note that hot counting pauses,
/// with no pause begun or ended: the optimizing tier's code then leaves
/// each link it comes to to the Vm, as it does during a pause. A real
/// pause begins at a collection exit and ends when a trace is entered;
/// one ending while the note is held leaves it up.
pub fn hold_run_words_quiet(vm: &Vm, on: bool) {
    vm.jit.words.quiet.set(u32::from(on));
    vm.jit.words.quiet_held.set(on);
}

/// Holds (`on`) or drops the run words' fourth blocked byte, which nothing
/// else sets: compiled code reading the word then leaves to the dispatcher
/// what it leaves when an exit left something for it, which finds nothing
/// to do.
pub fn hold_run_words_blocked(vm: &Vm, on: bool) {
    let b = crate::vm::jit_state::BLOCKED_HELD;
    vm.jit.words.block(b, on);
}

/// Lets (`on`, as by default) or keeps the baseline tier's call stub from
/// taking links itself, for the cells filled from now on: with it kept, the
/// stub leaves each link to the Vm, as it did before it could take one.
pub fn let_stub_take_links(vm: &Vm, on: bool) {
    vm.jit.words.stub_links_off.set(!on);
}
