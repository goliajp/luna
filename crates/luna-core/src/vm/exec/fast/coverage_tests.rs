//! Every arm of the fast loop, run as a function (an unoptimised build),
//! is dispatched by the corpus of `arm_tests`, and every kind of `Flow` an
//! arm can return without the trace JIT (`Exit` needs one) comes back.

#[test]
fn every_arm_is_dispatched() {
    use super::super::arm_tests::{in_time, run_all};
    // the record is the thread's own
    let seen = in_time(|| {
        super::take();
        run_all();
        super::take()
    });
    let missing: Vec<_> = super::ARMS
        .iter()
        .filter(|op| !seen.iter().any(|&(o, _)| o == **op as u8))
        .collect();
    assert!(missing.is_empty(), "arms never dispatched: {missing:?}");
    for kind in 0..7 {
        assert!(
            seen.iter().any(|&(_, k)| k == kind),
            "no arm returned flow {kind}"
        );
    }
}
