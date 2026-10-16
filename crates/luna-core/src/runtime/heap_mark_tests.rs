//! The marker on graphs a recursive or growing gray stack would not
//! manage; the order an incremental sweep leaves.

use super::*;

#[test]
fn deep_table_chain_marks_iteratively() {
    // deep chain: explicit mark stack must not overflow (smaller under
    // miri — the interpreter makes 100k tables take ~30 minutes)
    let n = if cfg!(miri) { 2_000 } else { 100_000 };
    let mut heap = Heap::new();
    let head = heap.new_table();
    let mut cur = head;
    for _ in 0..n {
        let next = heap.new_table();
        // SAFETY: `cur` is `head` or the table allocated in the previous iteration, and nothing
        // collects inside the loop; the borrow covers one call
        unsafe { cur.as_mut() }
            .set(&mut heap, Value::Int(1), Value::Table(next))
            .unwrap();
        cur = next;
    }
    assert_eq!(heap.collect(&[Value::Table(head)]), 0);
    assert_eq!(heap.collect(&[]), n + 1);
}

/// Refuses every growth once armed.
struct Refuse(std::rc::Rc<std::cell::Cell<bool>>);

impl crate::runtime::mem::MemoryPolicy for Refuse {
    fn allow(&mut self, _: usize, _: usize, _: crate::runtime::mem::BlockKind, _: usize) -> bool {
        !self.0.get()
    }
}

#[test]
fn a_gray_stack_that_cannot_grow_still_marks_everything() {
    // a wide, deep graph: marking it needs a gray stack far larger than the
    // one a fresh heap has; with every allocation refused the marker finds
    // the objects it could not stack by walking the object list
    let armed = std::rc::Rc::new(std::cell::Cell::new(false));
    let mem = crate::runtime::mem::MemOwner::policy(Box::new(Refuse(armed.clone())));
    let mut heap = Heap::with_mem(mem, 1);
    let root = heap.new_table();
    let mut kept = 1;
    for i in 0..200 {
        let branch = heap.new_table();
        let mut cur = branch;
        for _ in 0..50 {
            let next = heap.new_table();
            // SAFETY: `cur` was allocated in this loop and nothing collects inside it
            unsafe { cur.as_mut() }
                .set(&mut heap, Value::Int(1), Value::Table(next))
                .unwrap();
            cur = next;
        }
        // SAFETY: `root` is held by this test and nothing collects inside the loop
        unsafe { root.as_mut() }
            .set(&mut heap, Value::Int(i + 1), Value::Table(branch))
            .unwrap();
        kept += 51;
    }
    let garbage = 500;
    for _ in 0..garbage {
        heap.new_table();
    }
    let before = heap.live_objects();
    armed.set(true);
    assert_eq!(heap.collect(&[Value::Table(root)]), garbage);
    assert_eq!(heap.live_objects(), before - garbage);
    assert!(heap.live_objects() >= kept);
    // every kept table is still reachable and intact
    let mut cur = root.get_int(200);
    for _ in 0..50 {
        let Value::Table(t) = cur else {
            panic!("chain broken")
        };
        cur = t.get_int(1);
    }
    armed.set(false);
    assert_eq!(heap.collect(&[]), before - garbage - 1);
}

/// The objects on `all` in list order.
fn list_order(heap: &Heap) -> Vec<usize> {
    let mut out = Vec::new();
    let mut cur = heap.all;
    while !cur.is_null() {
        out.push(cur as usize);
        // SAFETY: `all` links objects this heap allocated and has not freed
        cur = unsafe { (*cur).next };
    }
    out
}

#[test]
fn incremental_sweep_keeps_the_survivors_in_order() {
    let mut heap = Heap::new();
    let tables: Vec<_> = (0..40).map(|_| heap.new_table()).collect();
    // every other one stays
    let roots: Vec<Value> = tables.iter().step_by(2).map(|&t| Value::Table(t)).collect();
    let kept: Vec<usize> = tables
        .iter()
        .step_by(2)
        .map(|t| t.as_ptr() as usize)
        .collect();
    heap.gc_start_propagate(&roots, &[]);
    while !heap.gc_step_propagate(usize::MAX) {}
    heap.gc_finish_atomic();
    // made during the sweep: in front of where it is, never swept
    let young = heap.new_table();
    while !heap.gc_sweep_step(3) {}
    let order = list_order(&heap);
    assert_eq!(order[0], young.as_ptr() as usize);
    let tables_left: Vec<usize> = order.into_iter().filter(|p| kept.contains(p)).collect();
    let mut want = kept.clone();
    // `all` is newest first
    want.reverse();
    assert_eq!(tables_left, want);
}
