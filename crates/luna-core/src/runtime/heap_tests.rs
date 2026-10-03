use super::*;
use crate::vm::isa::{Inst, Op};

#[test]
fn collect_traces_function_objects() {
    let mut heap = Heap::new();
    let source = heap.intern(b"@test");
    let kstr = heap.intern(b"a-constant-string");
    let inner = Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: Box::new([Inst::iabc(Op::Return0, 0, 0, 0, false)]),
        consts: Box::new([]),
        protos: Box::new([]),
        upvals: Box::new([]),
        num_params: 0,
        is_vararg: false,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack: 2,
        lines: Box::new([1]),
        source,
        line_defined: 1,
        last_line_defined: 1,
        locvars: Box::new([]),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx: u8::MAX,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
        inlined_protos: std::cell::RefCell::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        has_dispatchable_trace: std::cell::Cell::new(false),
        trace_heads: std::cell::Cell::new(
            [crate::runtime::function::TRACE_HEADS_NONE; crate::runtime::function::TRACE_HEADS_CAP],
        ),
        trace_call_head_settled: std::cell::Cell::new(false),
    };
    let inner = heap.adopt_proto(inner);
    let outer = Proto {
        hdr: GcHeader::new(ObjTag::Proto),
        code: Box::new([Inst::iabc(Op::Return0, 0, 0, 0, false)]),
        consts: Box::new([Value::Str(kstr)]),
        protos: Box::new([inner]),
        upvals: Box::new([]),
        num_params: 0,
        is_vararg: true,
        has_vararg_table_pseudo: false,
        has_compat_vararg_arg: false,
        max_stack: 2,
        lines: Box::new([1]),
        source,
        line_defined: 0,
        last_line_defined: 0,
        locvars: Box::new([]),
        cache: std::cell::Cell::new(None),
        jit: std::cell::Cell::new(crate::runtime::function::JitProtoState::Untried),
        env_upval_idx: u8::MAX,
        trace_hot_count: std::cell::Cell::new(0),
        call_hot_count: std::cell::Cell::new(0),
        trace_discard_count: std::cell::Cell::new(0),
        trace_gave_up: std::cell::Cell::new(false),
        trace_compile_failures: crate::jit::send_compat::TRefLock::new(Vec::new()),
        inlined_protos: std::cell::RefCell::new(Vec::new()),
        traces: crate::jit::send_compat::TRefLock::new(Vec::new()),
        has_dispatchable_trace: std::cell::Cell::new(false),
        trace_heads: std::cell::Cell::new(
            [crate::runtime::function::TRACE_HEADS_NONE; crate::runtime::function::TRACE_HEADS_CAP],
        ),
        trace_call_head_settled: std::cell::Cell::new(false),
    };
    let outer = heap.adopt_proto(outer);
    let captured = heap.intern(b"captured-value-string-xxxxxxxxxxxxxxxxxxxxxxxxx");
    let uv = heap.new_upvalue(UpvalState::Closed(Value::Str(captured)));
    let cl = heap.new_closure(outer, Box::new([uv]));
    // objects: source, kstr, inner, outer, captured, uv, cl
    assert_eq!(heap.live_objects(), 7);
    // rooting the closure keeps the whole graph alive
    assert_eq!(heap.collect(&[Value::Closure(cl)]), 0);
    assert_eq!(heap.live_objects(), 7);
    assert_eq!(heap.collect(&[]), 7);
    assert_eq!(heap.live_objects(), 0);
}

#[test]
fn collect_unreachable() {
    let mut heap = Heap::new();
    let s = heap.intern(b"hello");
    let t = heap.new_table();
    assert_eq!(heap.live_objects(), 2);
    // both rooted: nothing freed
    assert_eq!(heap.collect(&[Value::Str(s), Value::Table(t)]), 0);
    // only table rooted: string freed
    assert_eq!(heap.collect(&[Value::Table(t)]), 1);
    assert_eq!(heap.live_objects(), 1);
    // nothing rooted
    assert_eq!(heap.collect(&[]), 1);
    assert_eq!(heap.live_objects(), 0);
}

#[test]
fn collect_traces_table_contents() {
    let mut heap = Heap::new();
    let t = heap.new_table();
    let k = heap.intern(b"key-string-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"); // long
    let v = heap.intern(b"val");
    // SAFETY: `t` was allocated above and is held by a local; nothing has collected since, and the
    // borrow covers one call
    unsafe { t.as_mut() }
        .set(&mut heap, Value::Str(k), Value::Str(v))
        .unwrap();
    let inner = heap.new_table();
    // SAFETY: `t` was allocated above and is held by a local; nothing has collected since, and the
    // borrow covers one call
    unsafe { t.as_mut() }
        .set(&mut heap, Value::Int(1), Value::Table(inner))
        .unwrap();
    // SAFETY: `inner` was allocated above and is held by a local; nothing has collected since, and
    // the borrow covers one call
    unsafe { inner.as_mut() }.set_metatable(Some(t));
    assert_eq!(heap.live_objects(), 4);
    // root only the outer table: everything reachable through it survives
    assert_eq!(heap.collect(&[Value::Table(t)]), 0);
    assert_eq!(heap.live_objects(), 4);
    assert_eq!(heap.collect(&[]), 4);
}

#[test]
fn interned_string_reclaimed_and_reinternable() {
    let mut heap = Heap::new();
    heap.intern(b"transient");
    assert_eq!(heap.collect(&[]), 1);
    let s2 = heap.intern(b"transient");
    assert_eq!(s2.as_bytes(), b"transient");
    assert_eq!(heap.live_objects(), 1);
}

#[test]
fn bytes_and_live_round_trip_to_zero() {
    // Memory-invariant audit: after a churn of table allocation, rehash-
    // driven growth, and full collection of an empty root set, both
    // `heap.bytes` and `heap.live_objects` must return to 0. Catches any
    // alloc / free asymmetry in the Table internal-Box delta tracking
    // or the live counter (link/sweep symmetry).
    let mut heap = Heap::new();
    assert_eq!(heap.bytes(), 0);
    assert_eq!(heap.live_objects(), 0);
    // Build a churn: 50 tables, each filled with 200 int keys (forces
    // multiple rehashes); plus interned strings spliced through the
    // hash part. Bytes should grow well past the empty baseline.
    let mut roots: Vec<Value> = Vec::new();
    for ti in 0..50 {
        let t = heap.new_table();
        for k in 1..=200 {
            // SAFETY: `t` was allocated at the top of this iteration and nothing collects inside
            // the loop; the borrow covers one call
            let _ = unsafe { t.as_mut() }.set(&mut heap, Value::Int(k), Value::Int(ti * 1000 + k));
        }
        for sk in 0..32 {
            let key = Value::Str(heap.intern(format!("k{ti}-{sk}").as_bytes()));
            // SAFETY: `t` was allocated at the top of this iteration and nothing collects inside
            // the loop; the borrow covers one call
            let _ = unsafe { t.as_mut() }.set(&mut heap, key, Value::Int(sk));
        }
        roots.push(Value::Table(t));
    }
    let live_peak = heap.live_objects();
    let bytes_peak = heap.bytes();
    assert!(live_peak > 0, "live should be >0 after churn");
    assert!(bytes_peak > 0, "bytes should be >0 after churn");
    // Root only half — the other half should be collected.
    let half = roots.len() / 2;
    let freed = heap.collect(&roots[..half]);
    assert!(freed > 0, "some objects should have been freed");
    assert!(
        heap.bytes() < bytes_peak,
        "bytes must drop after partial collect"
    );
    assert!(
        heap.live_objects() < live_peak,
        "live must drop after partial collect"
    );
    // Drop everything: counters must return to 0 exactly.
    drop(roots);
    let _ = heap.collect(&[]);
    assert_eq!(heap.live_objects(), 0, "live not zero after full collect");
    assert_eq!(
        heap.bytes(),
        0,
        "bytes not zero after full collect — asymmetric alloc/free"
    );
}

/// Regression test for a string-table use-after-free:
/// `StringTable::intern` must NOT return a dead-white (about-to-be-swept)
/// short-string pointer. Mirrors PUC `luaS_new`'s resurrect-on-hit guard
/// (lstring.c — `if (isdead(g, ts)) changewhite(ts);`).
///
/// Without the guard, the bucket-chain still references the unswept
/// short string after the atomic flip; a re-`intern` of the same bytes
/// hands back that pointer; the budget-paced sweep then frees it and
/// the next bucket walk dereferences libc-recycled garbage (the
/// `0x800002a80000002d` misaligned pointer, for example).
#[test]
fn intern_resurrects_dead_white_short_string() {
    let mut heap = Heap::new();
    let alive = heap.intern(b"keep-me-alive-1");
    let dying = heap.intern(b"transient-x");
    let dying_ptr = dying.as_ptr();
    let dying_bytes = dying.as_bytes().to_vec();
    // Drive an incremental cycle by hand to reproduce the race:
    //   1. mark-propagate with `alive` only as a root → `dying` stays white
    //   2. atomic flip → `dying` becomes dead-white, bucket still points at it
    //   3. RE-INTERN the same bytes BEFORE sweep clears the bucket
    //   4. fix must either (a) skip the dead entry & alloc fresh, or
    //      (b) resurrect dying back to current-white
    let alive_root = [Value::Str(alive)];
    heap.gc_start_propagate(&alive_root, &[]);
    while !heap.gc_step_propagate(usize::MAX) {}
    heap.gc_finish_atomic();
    // At this point sweep_cur holds the detached old-heap list and the
    // dying string is dead-white. The bucket chain in `self.strings`
    // still references it.
    let resurrected = heap.intern(&dying_bytes);
    // Two valid outcomes:
    //   * resurrect: same pointer, but flagged current-white so sweep
    //     will keep it alive (PUC luaS_new shape)
    //   * skip-and-alloc-fresh: different pointer, dying gets swept
    //     normally as it should
    // Either way, after completing the sweep the heap must NOT crash
    // when we try to intern more short strings (which walks bucket chains).
    while !heap.gc_sweep_step(usize::MAX) {}
    // Smoke: bucket-chain walk for a fresh string must not deref a
    // freed pointer.
    let _fresh = heap.intern(b"after-sweep-canary");
    // If the fix is "resurrect", same pointer + bytes preserved:
    if resurrected.as_ptr() == dying_ptr {
        assert_eq!(resurrected.as_bytes(), dying_bytes.as_slice());
    }
    // Final cleanup: full collect must complete without UAF.
    drop(heap);
}

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
