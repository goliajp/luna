use crate::runtime::heap::{Gc, Heap};
use crate::runtime::{Table, UserdataPayload, Value};

fn noop(_: &mut crate::vm::Vm, _: u32, _: u32) -> Result<u32, crate::vm::LuaError> {
    Ok(0)
}

/// One fresh object of every collectable kind, from the lowest tag
/// (string) to the highest (userdata). The coroutine also keeps `globals`.
fn one_of_each(heap: &mut Heap, globals: Gc<Table>, n: usize) -> Vec<Value> {
    let s =
        heap.intern(format!("a string long enough not to be interned, number {n:04}").as_bytes());
    vec![
        Value::Str(s),
        Value::Table(heap.new_table()),
        Value::Native(heap.new_native(noop, Box::new([]))),
        Value::Coro(heap.new_coro(Value::Nil, globals)),
        Value::Userdata(heap.new_userdata(UserdataPayload::Empty, false)),
    ]
}

/// A host pointer the collector must never treat as an object: marking
/// it would write through an unmapped address.
fn light() -> Value {
    Value::LightUserdata(16 as *const ())
}

fn set(heap: &mut Heap, t: Gc<Table>, k: Value, v: Value) {
    // SAFETY: test-only; no other reference to `t` is live
    unsafe { t.as_mut() }.set(heap, k, v).unwrap();
}

fn with_mode(heap: &mut Heap, t: Gc<Table>, mode: &[u8]) {
    let mt = heap.new_table();
    let k = Value::Str(heap.intern(b"__mode"));
    let v = Value::Str(heap.intern(mode));
    set(heap, mt, k, v);
    // SAFETY: as in `set`
    unsafe { t.as_mut() }.set_metatable(Some(mt));
}

#[test]
fn strong_table_marks_every_collectable_slot() {
    let mut heap = Heap::new();
    // the fixed "not enough memory" string is never collected
    let base = heap.live_objects();
    let globals = heap.new_table();
    let t = heap.new_table_sized(8);
    // array part: every kind, a light userdata, a number
    let arr = one_of_each(&mut heap, globals, 0);
    for (i, &v) in arr.iter().enumerate() {
        set(&mut heap, t, Value::Int(i as i64 + 1), v);
    }
    set(&mut heap, t, Value::Int(6), light());
    set(&mut heap, t, Value::Int(7), Value::Float(1.5));
    assert_eq!(t.asize, 8, "values belong in the array part");
    // hash part: every kind as a key, every kind as a value
    for (i, k) in one_of_each(&mut heap, globals, 1).into_iter().enumerate() {
        set(&mut heap, t, k, Value::Int(i as i64));
    }
    for (i, v) in one_of_each(&mut heap, globals, 2).into_iter().enumerate() {
        set(&mut heap, t, Value::Int(100 + i as i64), v);
    }
    set(&mut heap, t, light(), light());
    let live = heap.live_objects();
    assert_eq!(heap.collect(&[Value::Table(t), Value::Table(globals)]), 0);
    assert_eq!(heap.live_objects(), live);
    // the coroutines keep `globals`; without them nothing survives
    assert_eq!(heap.collect(&[]), live - base);
}

#[test]
fn weak_value_table_marks_its_keys() {
    let mut heap = Heap::new();
    let globals = heap.new_table();
    let t = heap.new_table_sized(4);
    with_mode(&mut heap, t, b"v");
    for (i, k) in one_of_each(&mut heap, globals, 0).into_iter().enumerate() {
        set(&mut heap, t, k, Value::Int(i as i64));
    }
    // weak values reachable only from here: collected, except the string
    // (strings are values, never weakly cleared)
    let dropped = one_of_each(&mut heap, globals, 1);
    for (i, &v) in dropped.iter().enumerate() {
        set(&mut heap, t, Value::Int(100 + i as i64), v);
    }
    let in_array = Value::Table(heap.new_table());
    set(&mut heap, t, Value::Int(1), in_array);
    let live = heap.live_objects();
    let freed = heap.collect(&[Value::Table(t), Value::Table(globals)]);
    assert_eq!(freed, dropped.len() - 1 + 1);
    assert_eq!(heap.live_objects(), live - freed);
    assert!(t.get(Value::Int(1)).is_nil());
}

#[test]
fn weak_key_table_without_ephemerons_marks_its_values() {
    let mut heap = Heap::new();
    heap.no_ephemeron = true;
    let globals = heap.new_table();
    let t = heap.new_table_sized(8);
    with_mode(&mut heap, t, b"k");
    let arr = one_of_each(&mut heap, globals, 0);
    for (i, &v) in arr.iter().enumerate() {
        set(&mut heap, t, Value::Int(i as i64 + 1), v);
    }
    for (i, v) in one_of_each(&mut heap, globals, 1).into_iter().enumerate() {
        set(&mut heap, t, Value::Int(100 + i as i64), v);
    }
    let live = heap.live_objects();
    assert_eq!(heap.collect(&[Value::Table(t), Value::Table(globals)]), 0);
    assert_eq!(heap.live_objects(), live);
}

#[test]
fn ephemeron_table_marks_values_of_live_keys_only() {
    let mut heap = Heap::new();
    let globals = heap.new_table();
    let t = heap.new_table_sized(8);
    with_mode(&mut heap, t, b"k");
    let arr = one_of_each(&mut heap, globals, 0);
    for (i, &v) in arr.iter().enumerate() {
        set(&mut heap, t, Value::Int(i as i64 + 1), v);
    }
    let keys = one_of_each(&mut heap, globals, 1);
    for &k in &keys {
        let v = Value::Table(heap.new_table());
        set(&mut heap, t, k, v);
    }
    // a key reachable only from here, with a value reachable only from it
    let lone = Value::Table(heap.new_table());
    let lone_val = Value::Table(heap.new_table());
    set(&mut heap, t, lone, lone_val);
    let mut roots = keys.clone();
    roots.push(Value::Table(t));
    roots.push(Value::Table(globals));
    assert_eq!(heap.collect(&roots), 2);
    for &k in &keys {
        assert!(matches!(t.get(k), Value::Table(_)));
    }
}

#[test]
fn native_upvalues_are_marked_through_a_table() {
    let mut heap = Heap::new();
    // the fixed "not enough memory" string is never collected
    let base = heap.live_objects();
    let t = heap.new_table();
    let up = heap.new_table();
    let s = heap.intern(b"an upvalue string long enough not to be interned at all");
    let f = heap.new_native(noop, Box::new([Value::Table(up), Value::Str(s)]));
    let bare = heap.new_native(noop, Box::new([]));
    set(&mut heap, t, Value::Int(1), Value::Native(f));
    set(&mut heap, t, Value::Int(2), Value::Native(bare));
    let live = heap.live_objects();
    assert_eq!(heap.collect(&[Value::Table(t)]), 0);
    assert_eq!(heap.live_objects(), live);
    assert_eq!(heap.collect(&[]), live - base);
}
