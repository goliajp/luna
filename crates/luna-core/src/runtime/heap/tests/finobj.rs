//! Objects with finalizers on lists of their own: off `all` to `finobj`
//! as they get one (from 5.2 on), to the end of `tobefnz` when found
//! unreachable, and back where PUC puts a finalized object
//! (`udata2finalize`: the head of `all`; 5.1's `GCTM`: after the main
//! thread) when the finalizer is taken.

use super::*;
use crate::runtime::userdata::UserdataPayload;
use crate::version::LuaVersion;

use super::mark::list_order as order;

fn at<T>(t: Gc<T>) -> usize {
    t.as_ptr() as usize
}

fn chain(mut cur: *mut GcHeader) -> Vec<usize> {
    let mut out = Vec::new();
    while !cur.is_null() {
        out.push(cur as usize);
        // SAFETY: the heap's lists link objects it allocated and has not freed
        cur = unsafe { (*cur).next };
    }
    out
}

fn heap_of(v: LuaVersion) -> Heap {
    let mut heap = Heap::new();
    heap.set_version(v).unwrap();
    heap
}

/// The marking of `roots` is over: the atomic step moved the unreachable
/// objects with finalizers to `tobefnz`, and the sweep has begun.
fn atomic_done(heap: &mut Heap, roots: &[Value]) {
    heap.gc_start_propagate(roots, &[]);
    while !heap.gc_step_propagate(usize::MAX) {}
    heap.gc_finish_atomic();
    assert!(heap.phase() == GcPhase::Sweep, "the sweep has begun");
}

fn finalizable(heap: &mut Heap) -> Gc<Table> {
    let t = heap.new_table();
    heap.register_finalizable(t);
    t
}

#[test]
fn an_object_given_a_finalizer_leaves_the_list_for_finobj() {
    let mut heap = heap_of(LuaVersion::Lua54);
    let old = heap.new_table();
    let rest: Vec<_> = (0..3).map(|_| heap.new_table()).collect();
    heap.register_finalizable(old);
    let o = order(&heap);
    assert!(!o.contains(&at(old)), "off `all`");
    assert_eq!(
        o[..3],
        [at(rest[2]), at(rest[1]), at(rest[0])],
        "the rest in order"
    );
    let newer = finalizable(&mut heap);
    assert_eq!(chain(heap.finobj), [at(newer), at(old)], "newest first");
    assert_eq!(heap.finobj_count(), 2);
}

#[test]
fn a_5_1_userdata_given_a_metatable_stays_after_the_main_thread() {
    let mut heap = heap_of(LuaVersion::Lua51);
    let g = heap.new_table();
    let main = heap.new_coro(Value::Nil, g);
    heap.main_thread_made(main);
    let u = heap.new_userdata(UserdataPayload::Empty, false);
    heap.register_finalizable_userdata(u);
    assert!(heap.finobj.is_null());
    let o = order(&heap);
    let m = o.iter().position(|&p| p == at(main)).unwrap();
    assert_eq!(o[m + 1], at(u));
}

#[test]
fn a_sweep_standing_on_the_object_sweeps_on_to_the_next_one_it_keeps() {
    let mut heap = heap_of(LuaVersion::Lua54);
    let keep = heap.new_table();
    let dead: Vec<_> = (0..3).map(|_| heap.new_table()).collect();
    let t = heap.new_table();
    atomic_done(&mut heap, &[Value::Table(t), Value::Table(keep)]);
    // the sweep began at the head, kept `t`, and stands on it
    assert_eq!(heap.sweep_prev, t.as_ptr() as *mut GcHeader);
    let live = heap.live_objects();
    heap.register_finalizable(t);
    assert_eq!(
        heap.live_objects(),
        live - dead.len(),
        "the dead ones before `keep` freed"
    );
    assert_eq!(heap.sweep_prev, keep.as_ptr() as *mut GcHeader);
    assert!(!order(&heap).contains(&at(t)));
}

#[test]
fn the_unreachable_ones_go_to_tobefnz_in_the_order_of_finobj() {
    let mut heap = heap_of(LuaVersion::Lua54);
    let a = finalizable(&mut heap);
    let kept = finalizable(&mut heap);
    let b = finalizable(&mut heap);
    // a sweep to the first object kept has one to stop at
    let plain = heap.new_table();
    atomic_done(&mut heap, &[Value::Table(kept), Value::Table(plain)]);
    assert_eq!(chain(heap.tobefnz), [at(b), at(a)]);
    assert_eq!(chain(heap.finobj), [at(kept)]);
    assert_eq!(heap.tobefnz_count(), 2);
}

#[test]
fn a_finalizer_taken_puts_its_object_back_at_the_head() {
    let mut heap = heap_of(LuaVersion::Lua54);
    let fin = finalizable(&mut heap);
    let keep = heap.new_table();
    atomic_done(&mut heap, &[Value::Table(keep)]);
    while !heap.gc_sweep_step(usize::MAX) {}
    let newer = heap.new_table();
    assert!(heap.take_one_tobefnz().is_some());
    let o = order(&heap);
    assert_eq!(
        o[..2],
        [at(fin), at(newer)],
        "in front of what was made after"
    );
    assert!(heap.tobefnz.is_null() && heap.tobefnz_end.is_null());
    let f = fin.hdr().flags();
    assert!(
        f & FIN == 0 && is_white(f) && f & heap.current_white != 0,
        "{f:#x}"
    );
}

#[test]
fn an_object_put_back_while_the_sweep_runs_is_not_swept_again() {
    let mut heap = heap_of(LuaVersion::Lua54);
    let older: Vec<_> = (0..3).map(|_| heap.new_table()).collect();
    let fin = finalizable(&mut heap);
    let first = heap.new_table();
    let mut roots: Vec<Value> = older.iter().map(|&t| Value::Table(t)).collect();
    roots.push(Value::Table(first));
    atomic_done(&mut heap, &roots);
    let _ = heap.sweep_objects(1, false);
    assert!(heap.take_one_tobefnz().is_some());
    assert_eq!(order(&heap)[0], at(fin));
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
fn a_finalized_5_1_userdata_goes_back_after_the_main_thread() {
    let mut heap = heap_of(LuaVersion::Lua51);
    let g = heap.new_table();
    let main = heap.new_coro(Value::Nil, g);
    heap.main_thread_made(main);
    let u = heap.new_userdata(UserdataPayload::Empty, false);
    heap.register_finalizable_userdata(u);
    atomic_done(&mut heap, &[Value::Table(g), Value::Coro(main)]);
    assert_eq!(chain(heap.tobefnz), [at(u)], "off the list");
    while !heap.gc_sweep_step(usize::MAX) {}
    // a userdata made since goes after the main thread, in front of `u`
    let u2 = heap.new_userdata(UserdataPayload::Empty, false);
    assert!(heap.take_one_tobefnz().is_some());
    let o = order(&heap);
    let m = o.iter().position(|&p| p == at(main)).unwrap();
    assert_eq!(
        o[m + 1..m + 3],
        [at(u), at(u2)],
        "right after the main thread"
    );
}

#[test]
fn a_heap_dropped_with_objects_on_finobj_and_tobefnz_frees_them() {
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
        let keep = finalizable(&mut heap);
        for _ in 0..8 {
            finalizable(&mut heap);
        }
        let plain = heap.new_table();
        atomic_done(&mut heap, &[Value::Table(keep), Value::Table(plain)]);
        assert!(!heap.finobj.is_null() && !heap.tobefnz.is_null());
    }
    assert_eq!(live.0.get(), 0, "every block given back");
}

#[test]
fn a_marking_dropped_for_a_full_collection_frees_nothing_and_finds_nothing_due() {
    let mut heap = heap_of(LuaVersion::Lua52);
    let fin = finalizable(&mut heap);
    let garbage = heap.new_table();
    let keep = heap.new_table();
    heap.gc_start_propagate(&[Value::Table(keep), Value::Table(fin)], &[]);
    while !heap.gc_step_propagate(usize::MAX) {}
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
