use super::*;
use crate::runtime::heap::Heap;

#[path = "table_length_tests.rs"]
mod length;

pub(super) fn with_table(f: impl FnOnce(&mut Heap, &mut Table)) {
    let mut heap = Heap::new();
    let t = heap.new_table();
    // SAFETY: `t` was just allocated and is held only by this local; the closure gets the only
    // reference to it, and no caller collects while it runs
    f(&mut heap, unsafe { t.as_mut() });
}

fn assert_is_border(t: &Table, n: i64) {
    if n == 0 {
        assert!(t.get_int(1).is_nil(), "border 0 but t[1] non-nil");
    } else {
        assert!(!t.get_int(n).is_nil(), "border {n} but t[{n}] is nil");
        assert!(
            t.get_int(n + 1).is_nil(),
            "border {n} but t[{}] non-nil",
            n + 1
        );
    }
}

/// The words the luna-jit table-field IC loads: the node pointer and
/// the node mask, at the offsets `jit_layout` gives.
#[test]
fn node_layout_pinned() {
    use jit_layout::*;
    assert_eq!(NODE_KEY_OFFSET, 0);
    assert_eq!(NODE_VAL_OFFSET, 16);
    assert_eq!(SIZEOF_NODE, 32);
    with_table(|heap, t| {
        // SAFETY: both offsets are fields of `Table`, read as the IC reads them
        let words = |t: &Table| unsafe {
            let base = t as *const Table as *const u8;
            (
                *(base.add(TABLE_NODES_OFFSET) as *const usize),
                *(base.add(TABLE_NODE_MASK_OFFSET) as *const u32),
            )
        };
        assert_eq!(words(t).1, u32::MAX, "empty hash part");
        for i in 0..3 {
            let k = Value::Str(heap.intern(format!("k{i}").as_bytes()));
            t.set(heap, k, Value::Int(i)).unwrap();
        }
        let (ptr, mask) = words(t);
        assert_eq!(ptr, t.nodes().as_ptr() as usize);
        assert_eq!(mask, t.nodes().len() as u32 - 1);
        assert_eq!(t.nodes().len(), 4);
    });
}

#[test]
fn sequence_grows_into_array() {
    with_table(|heap, t| {
        for i in 1..=1000 {
            let _ = t.set_int(heap, i, Value::Int(i * 10));
        }
        for i in 1..=1000 {
            assert!(t.get_int(i).raw_eq(Value::Int(i * 10)));
        }
        assert_eq!(t.len(), 1000);
    });
}

#[test]
fn string_and_mixed_keys() {
    with_table(|heap, t| {
        let k1 = Value::Str(heap.intern(b"alpha"));
        let k2 = Value::Str(heap.intern(b"beta"));
        t.set(heap, k1, Value::Int(1)).unwrap();
        t.set(heap, k2, Value::Int(2)).unwrap();
        t.set(heap, Value::Bool(true), Value::Int(3)).unwrap();
        t.set(heap, Value::Int(-5), Value::Int(4)).unwrap();
        // re-interned key reaches the same slot
        let k1b = Value::Str(heap.intern(b"alpha"));
        assert!(t.get(k1b).raw_eq(Value::Int(1)));
        assert!(t.get(k2).raw_eq(Value::Int(2)));
        assert!(t.get(Value::Bool(true)).raw_eq(Value::Int(3)));
        assert!(t.get(Value::Int(-5)).raw_eq(Value::Int(4)));
        assert!(t.get(Value::Str(heap.intern(b"gamma"))).is_nil());
    });
}

#[test]
fn float_keys_normalize_to_int() {
    with_table(|heap, t| {
        t.set(heap, Value::Float(2.0), Value::Int(22)).unwrap();
        assert!(t.get(Value::Int(2)).raw_eq(Value::Int(22)));
        t.set(heap, Value::Int(3), Value::Int(33)).unwrap();
        assert!(t.get(Value::Float(3.0)).raw_eq(Value::Int(33)));
        // -0.0 is key 0
        t.set(heap, Value::Float(-0.0), Value::Int(0)).unwrap();
        assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
        // non-integral floats are their own keys
        t.set(heap, Value::Float(0.5), Value::Int(55)).unwrap();
        assert!(t.get(Value::Float(0.5)).raw_eq(Value::Int(55)));
        assert!(t.get(Value::Int(0)).raw_eq(Value::Int(0)));
    });
}

#[test]
fn bad_keys() {
    with_table(|heap, t| {
        assert_eq!(
            t.set(heap, Value::Nil, Value::Int(1)),
            Err(TableError::NilIndex)
        );
        assert_eq!(
            t.set(heap, Value::Float(f64::NAN), Value::Int(1)),
            Err(TableError::NanIndex)
        );
        // reads with bad keys are nil, not errors
        assert!(t.get(Value::Nil).is_nil());
        assert!(t.get(Value::Float(f64::NAN)).is_nil());
    });
}

#[test]
fn delete_and_reinsert() {
    with_table(|heap, t| {
        let k = Value::Str(heap.intern(b"k"));
        t.set(heap, k, Value::Int(1)).unwrap();
        t.set(heap, k, Value::Nil).unwrap();
        assert!(t.get(k).is_nil());
        t.set(heap, k, Value::Int(2)).unwrap();
        assert!(t.get(k).raw_eq(Value::Int(2)));
        // setting an absent key to nil stays absent
        let k2 = Value::Str(heap.intern(b"k2"));
        t.set(heap, k2, Value::Nil).unwrap();
        assert!(t.get(k2).is_nil());
    });
}

#[test]
fn borders_with_holes() {
    with_table(|heap, t| {
        let _ = t.set_int(heap, 1, Value::Int(1));
        let _ = t.set_int(heap, 2, Value::Int(2));
        assert_eq!(t.len(), 2);
        t.set_int(heap, 2, Value::Nil).unwrap();
        assert_is_border(t, t.len());
        // hash-resident tail
        let _ = t.set_int(heap, 1_000_000, Value::Int(1));
        assert_is_border(t, t.len());
    });
}

#[test]
fn len_on_empty_and_hash_only() {
    with_table(|heap, t| {
        assert_eq!(t.len(), 0);
        let xk = Value::Str(heap.intern(b"x"));
        t.set(heap, xk, Value::Int(1)).unwrap();
        assert_eq!(t.len(), 0);
    });
}

#[test]
fn next_iterates_everything_exactly_once() {
    with_table(|heap, t| {
        let mut expected = 0i64;
        for i in 1..=64 {
            let _ = t.set_int(heap, i, Value::Int(i));
            expected += i;
        }
        for i in 0..32 {
            let k = Value::Str(heap.intern(format!("s{i}").as_bytes()));
            t.set(heap, k, Value::Int(1000 + i)).unwrap();
            expected += 1000 + i;
        }
        t.set(heap, Value::Float(2.5), Value::Int(7)).unwrap();
        expected += 7;

        let mut sum = 0i64;
        let mut count = 0;
        let mut key = Value::Nil;
        while let Some((k, v)) = t.next(key).unwrap() {
            let Value::Int(x) = v else {
                panic!("bad value")
            };
            sum += x;
            count += 1;
            key = k;
        }
        assert_eq!(count, 64 + 32 + 1);
        assert_eq!(sum, expected);
    });
}

#[test]
fn next_skips_nil_values_and_rejects_alien_keys() {
    with_table(|heap, t| {
        let _ = t.set_int(heap, 1, Value::Int(1));
        let _ = t.set_int(heap, 3, Value::Int(3));
        let k = Value::Str(heap.intern(b"gone"));
        t.set(heap, k, Value::Int(9)).unwrap();
        t.set(heap, k, Value::Nil).unwrap();
        let mut seen = Vec::new();
        let mut key = Value::Nil;
        while let Some((k, v)) = t.next(key).unwrap() {
            let Value::Int(x) = v else { panic!() };
            seen.push(x);
            key = k;
        }
        assert_eq!(seen, vec![1, 3]);
        // a key never inserted is invalid for next
        let alien = Value::Str(heap.intern(b"never"));
        assert!(matches!(t.next(alien), Err(TableError::InvalidNext)));
        // ...but a deleted (nil-valued) key is still a valid cursor
        assert!(t.next(k).is_ok());
    });
}

#[test]
fn collision_relocation_keeps_chains_intact() {
    with_table(|heap, t| {
        // dense negative ints all land in the hash part; with identity
        // hashing they exercise both chain cases heavily
        for i in 0..512 {
            let _ = t.set_int(heap, -i, Value::Int(i));
        }
        for i in 0..512 {
            assert!(t.get_int(-i).raw_eq(Value::Int(i)), "lost key {}", -i);
        }
    });
}

#[test]
fn rehash_redistributes_into_array() {
    with_table(|heap, t| {
        // insert 1..n in reverse: starts in hash, rehash must migrate
        for i in (1..=256).rev() {
            let _ = t.set_int(heap, i, Value::Int(i));
        }
        assert_eq!(t.len(), 256);
        for i in 1..=256 {
            assert!(t.get_int(i).raw_eq(Value::Int(i)));
        }
    });
}

/// What a table's layout and contents look like from outside: array and
/// hash sizes, the length, and the `next` order.
fn shape(t: &Table) -> String {
    let mut out = format!(
        "asize {} nodes {} len {} |",
        t.asize(),
        t.nodes().len(),
        t.len()
    );
    let mut k = Value::Nil;
    while let Some((nk, v)) = t.next(k).unwrap() {
        out.push_str(&format!(" {nk:?}={v:?}"));
        k = nk;
    }
    out
}

/// Run `ops` on two fresh tables of one heap (so strings hash alike), one
/// with the append shortcut and one with the full rehash only; the shapes
/// after every step must agree. Returns how often the shortcut ran.
fn same_as_full_rehash(ops: &[&dyn Fn(&mut Heap, &mut Table)]) -> u32 {
    let mut heap = Heap::new();
    let a = heap.new_table();
    let b = heap.new_table();
    // SAFETY: two distinct live tables of this heap, used one at a time
    let (a, b) = unsafe { (a.as_mut(), b.as_mut()) };
    grow::APPEND_REHASHES.with(|c| c.set(0));
    for (i, op) in ops.iter().enumerate() {
        op(&mut heap, a);
        let taken = grow::APPEND_REHASHES.with(|c| c.get());
        grow::FULL_REHASH_ONLY.with(|c| c.set(true));
        op(&mut heap, b);
        grow::FULL_REHASH_ONLY.with(|c| c.set(false));
        assert_eq!(grow::APPEND_REHASHES.with(|c| c.get()), taken);
        assert_eq!(shape(a), shape(b), "step {i}");
    }
    grow::APPEND_REHASHES.with(|c| c.get())
}

fn append(n: i64) -> impl Fn(&mut Heap, &mut Table) {
    move |heap, t| {
        for _ in 0..n {
            let k = t.len() + 1;
            t.set_int(heap, k, Value::Int(k)).unwrap();
        }
    }
}

fn put(k: i64, v: Value) -> impl Fn(&mut Heap, &mut Table) {
    move |heap, t| t.set_int(heap, k, v).unwrap()
}

#[test]
fn appending_past_a_full_array_matches_the_full_rehash() {
    let taken = same_as_full_rehash(&[&append(300)]);
    assert!(taken >= 8, "the shortcut ran {taken} times");
    // holes: the length then picks some border, and a refilled hole
    same_as_full_rehash(&[
        &append(40),
        &put(10, Value::Nil),
        &put(20, Value::Nil),
        &append(30),
        &put(10, Value::Int(1)),
        &append(100),
    ]);
    // keys in the hash part first, then the array fills under them
    same_as_full_rehash(&[
        &put(100, Value::Int(1)),
        &put(5, Value::Int(1)),
        &append(4),
        &append(200),
    ]);
    // a string key and a shrink: the upper half emptied, then rehashed
    // by hash inserts, then appended to again
    same_as_full_rehash(&[
        &append(64),
        &|heap: &mut Heap, t: &mut Table| {
            for k in 33..=64 {
                t.set_int(heap, k, Value::Nil).unwrap();
            }
            for i in 0..40 {
                let s = Value::Str(heap.intern(format!("s{i}").as_bytes()));
                t.set(heap, s, Value::Int(i)).unwrap();
            }
        },
        &append(70),
    ]);
    // an array part that is not a power of two
    same_as_full_rehash(&[
        &|heap: &mut Heap, t: &mut Table| t.ensure_array(heap, 3).unwrap(),
        &append(3),
        &append(20),
    ]);
}

// 5.4's dense-array `#t` shortcut moves `alimit` exactly as the full
// search does, for every reachable `alimit` (in `(a/2, a]` for a
// power-of-two array part, the size itself otherwise)
#[test]
fn len_54_shortcut_matches_the_search() {
    for a in 1..=40usize {
        for p in 0..a {
            let lims: Vec<u32> = if a.is_power_of_two() {
                (a as u32 / 2 + 1..=a as u32).collect()
            } else {
                vec![a as u32]
            };
            for l in lims {
                let mut out = [(0i64, 0u32); 2];
                for (k, o) in out.iter_mut().enumerate() {
                    with_table(|heap, t| {
                        t.hdr.sub = Dialect::L54 as u8;
                        t.resize(heap, a, 0).unwrap();
                        for i in 1..=p {
                            let _ = t.set_int(heap, i as i64, Value::Int(1));
                        }
                        t.alimit.set(l);
                        let n = if k == 0 { t.len() } else { t.len_54() };
                        *o = (n, t.alimit.get());
                    });
                }
                assert_eq!(out[0], out[1], "a {a} p {p} alimit {l}");
            }
        }
    }
}
