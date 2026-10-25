//! Objects due for their finalizers: off the list once a sweep meets them,
//! and back where PUC puts a finalized object (`udata2finalize`: the head
//! of the list; 5.1's `GCTM`: after the main thread) when the finalizer is
//! taken, whichever comes first.

use super::*;
use crate::version::LuaVersion;

use super::mark::list_order as order;

fn at(t: Gc<Table>) -> usize {
    t.as_ptr() as usize
}

/// A heap of `v` whose marking of `roots` is over: the atomic step found
/// the unreachable finalizable objects due, and the sweep has begun.
fn atomic_done(heap: &mut Heap, roots: &[Value]) {
    heap.gc_start_propagate(roots, &[]);
    while !heap.gc_step_propagate(usize::MAX) {}
    heap.gc_finish_atomic();
    assert!(heap.phase() == GcPhase::Sweep, "the sweep has begun");
}

fn finalizable(heap: &mut Heap) -> Gc<Table> {
    let t = heap.new_table();
    heap.register_finalizable(t).unwrap();
    t
}

#[test]
fn a_sweep_parks_an_object_due_and_its_finalizer_puts_it_back_at_the_head() {
    let mut heap = Heap::new();
    heap.set_version(LuaVersion::Lua54).unwrap();
    let fin = finalizable(&mut heap);
    let keep = heap.new_table();
    atomic_done(&mut heap, &[Value::Table(keep)]);
    while !heap.gc_sweep_step(usize::MAX) {}
    assert!(!order(&heap).contains(&at(fin)), "parked off the list");
    let newer = heap.new_table();
    assert!(heap.take_one_tobefnz().is_some());
    let o = order(&heap);
    assert_eq!(o[0], at(fin), "back at the head");
    assert_eq!(o[1], at(newer), "in front of what was made after the sweep");
}

#[test]
fn a_finalizer_taken_before_the_sweep_meets_its_object_puts_it_back_there() {
    let mut heap = Heap::new();
    heap.set_version(LuaVersion::Lua54).unwrap();
    let fin = finalizable(&mut heap);
    let keep = heap.new_table();
    // the sweep into the list stops at `keep`, ahead of `fin`
    atomic_done(&mut heap, &[Value::Table(keep)]);
    assert!(heap.take_one_tobefnz().is_some());
    assert!(
        order(&heap).contains(&at(fin)),
        "not met yet: still in place"
    );
    while !heap.gc_sweep_step(usize::MAX) {}
    let o = order(&heap);
    assert_eq!(
        &o[..2],
        &[at(fin), at(keep)],
        "the sweep put it back at the head"
    );
}

#[test]
fn an_object_put_back_while_the_sweep_runs_is_not_swept_again() {
    let mut heap = Heap::new();
    heap.set_version(LuaVersion::Lua54).unwrap();
    let older: Vec<_> = (0..3).map(|_| heap.new_table()).collect();
    let fin = finalizable(&mut heap);
    let first = heap.new_table();
    let mut roots: Vec<Value> = older.iter().map(|&t| Value::Table(t)).collect();
    roots.push(Value::Table(first));
    atomic_done(&mut heap, &roots);
    // one object: the sweep parks `fin` and keeps the next one
    let _ = heap.sweep_objects(1, false);
    assert!(!order(&heap).contains(&at(fin)));
    assert!(heap.take_one_tobefnz().is_some());
    assert_eq!(order(&heap)[0], at(fin));
    // what is left: the two oldest tables and what the heap made first
    let left = order(&heap)
        .iter()
        .skip_while(|&&p| p != at(older[1]))
        .count();
    assert_eq!(
        heap.sweep_objects(usize::MAX, false),
        left,
        "behind the sweep"
    );
}

#[test]
fn a_heap_dropped_with_parked_objects_frees_them() {
    #[derive(Default)]
    struct Live(std::cell::Cell<isize>);
    struct Track(std::rc::Rc<Live>);
    impl crate::runtime::mem::MemoryPolicy for Track {
        fn allow(
            &mut self,
            old: usize,
            new: usize,
            _: crate::runtime::mem::BlockKind,
            _: usize,
        ) -> bool {
            self.0.0.set(self.0.0.get() + new as isize - old as isize);
            true
        }
        fn freed(&mut self, size: usize) {
            self.0.0.set(self.0.0.get() - size as isize);
        }
    }
    let live = std::rc::Rc::new(Live::default());
    {
        let mem = crate::runtime::mem::MemOwner::policy(Box::new(Track(live.clone())));
        let mut heap = Heap::with_mem(mem, 1);
        heap.set_version(LuaVersion::Lua54).unwrap();
        let keep = heap.new_table();
        for _ in 0..8 {
            finalizable(&mut heap);
        }
        atomic_done(&mut heap, &[Value::Table(keep)]);
        while !heap.gc_sweep_step(usize::MAX) {}
        assert!(!heap.parked.is_null(), "parked when the heap goes");
    }
    assert_eq!(live.0.get(), 0, "every block given back");
}

#[test]
fn a_finalized_5_1_userdata_goes_back_after_the_main_thread() {
    let mut heap = Heap::new();
    heap.set_version(LuaVersion::Lua51).unwrap();
    let g = heap.new_table();
    let main = heap.new_coro(Value::Nil, g);
    heap.main_thread_made(main);
    let u = heap.new_userdata(crate::runtime::userdata::UserdataPayload::Empty, false);
    heap.register_finalizable_userdata(u).unwrap();
    atomic_done(&mut heap, &[Value::Table(g), Value::Coro(main)]);
    while !heap.gc_sweep_step(usize::MAX) {}
    // a userdata made since goes after the main thread, in front of `u`
    let u2 = heap.new_userdata(crate::runtime::userdata::UserdataPayload::Empty, false);
    assert!(heap.take_one_tobefnz().is_some());
    let o = order(&heap);
    let m = o.iter().position(|&p| p == main.as_ptr() as usize).unwrap();
    assert_eq!(
        &o[m + 1..m + 3],
        &[u.as_ptr() as usize, u2.as_ptr() as usize],
        "right after the main thread"
    );
}

#[test]
fn a_marking_dropped_for_a_full_collection_frees_nothing_and_finds_nothing_due() {
    let mut heap = Heap::new();
    heap.set_version(LuaVersion::Lua52).unwrap();
    let fin = finalizable(&mut heap);
    let garbage = heap.new_table();
    let keep = heap.new_table();
    heap.gc_start_propagate(&[Value::Table(keep)], &[]);
    let _ = heap.gc_step_propagate(1);
    let live = heap.live_objects();
    heap.gc_abandon_marking();
    assert!(heap.phase() == GcPhase::Pause);
    assert_eq!(heap.live_objects(), live, "nothing freed");
    assert!(!heap.has_tobefnz(), "no atomic step: nothing found due");
    for t in [fin, garbage, keep] {
        let f = t.hdr().flags();
        assert!(is_white(f) && f & heap.current_white != 0, "{f:#x}");
    }
}
