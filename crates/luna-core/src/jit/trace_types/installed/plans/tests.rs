use super::*;
use ExitTag::*;

fn inline(exits: &[(u32, &[ExitTag])]) -> ExitTagTable {
    ExitTagTable::from_exits(exits.iter().copied())
}

#[test]
fn exits_are_numbered_inline_then_typed_then_the_tail() {
    let typed = ExitTagTable::from_exits([(7, &[Int, Untouched][..])]);
    let p = TracePlans::new(
        &[3, 4],
        &[Int, Float],
        typed,
        inline(&[(9, &[Str, Nil, Bool])]),
        PassCount::Shared,
    );
    let q = &*p.0;
    assert_eq!(p.exit_count(), 3);
    assert_eq!(
        (q.exit_tags(0), q.exit_cont_pc(0)),
        (&[Str, Nil, Bool][..], Some(9))
    );
    assert_eq!(
        (q.exit_tags(1), q.exit_cont_pc(1)),
        (&[Int, Untouched][..], Some(7))
    );
    assert_eq!(
        (q.exit_tags(2), q.exit_cont_pc(2)),
        (&[Int, Float][..], None)
    );
    assert!(q.exit(3).is_none());
    // each exit is planned from its own tags
    for i in 0..3 {
        let e = q.exit(i).expect("an exit");
        let written = q.exit_tags(i).iter().filter(|&&t| t != Untouched).count();
        assert_eq!((e.len, e.written.len()), (q.exit_tags(i).len(), written));
    }
}

#[test]
fn the_size_stays_the_same_once_an_exit_is_planned() {
    let table = ExitTagTable::from_exits([(7, &[Int, Untouched][..])]);
    let p = TracePlans::new(
        &[3, 4],
        &[Int, Float],
        table,
        inline(&[(2, &[Untouched])]),
        PassCount::Shared,
    );
    let before = p.byte_size();
    let e = p.0.exit(1).expect("exit 1");
    assert_eq!(e.written.len(), 1);
    assert_eq!(p.byte_size(), before);
}

#[test]
fn threads_taking_an_exit_first_together_share_one_plan() {
    const N: usize = 8;
    let p = TracePlans::new(
        &[3],
        &[Int, Untouched, Float],
        ExitTagTable::default(),
        ExitTagTable::default(),
        PassCount::Shared,
    );
    let go = std::sync::Barrier::new(N);
    let got: Vec<usize> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..N)
            .map(|_| {
                s.spawn(|| {
                    go.wait();
                    let e = p.0.exit(0).expect("exit 0");
                    e as *const _ as usize
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().expect("thread")).collect()
    });
    assert!(got.iter().all(|&a| a == got[0]));
    let e = p.0.exit(0).expect("exit 0");
    assert_eq!((e.written.len(), e.untouched_end), (2, 2));
}

#[test]
fn plans_cross_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<TracePlans>();
}

fn counting(kinds: &[(u16, ExitTag)]) -> TracePlans {
    let none = ExitTagTable::default;
    TracePlans::new(&[], &[], none(), none(), PassCount::PerCall(kinds.into()))
}

#[test]
fn back_edge_kinds_by_register() {
    let p = counting(&[(9, Float), (2, Int), (4, Nil)]);
    assert_eq!(p.0.back_edge_kinds(), &[(2, Int), (4, Nil), (9, Float)]);
    let none = ExitTagTable::default;
    let p = TracePlans::new(&[], &[], none(), none(), PassCount::Shared);
    assert!(p.0.back_edge_kinds().is_empty());
}

#[test]
#[should_panic(expected = "not a kind the collector skips")]
fn a_collectable_back_edge_kind_is_refused() {
    let _ = counting(&[(0, Table)]);
}

#[test]
#[should_panic(expected = "two back edge kinds")]
fn a_register_given_twice_is_refused() {
    let _ = counting(&[(5, Int), (5, Float)]);
}
