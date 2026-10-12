//! A load, and a `load` with a reader function, refused at each allocation
//! in turn: the outcome is always the memory error, nothing is left
//! behind, and the vm goes on working.

use std::cell::Cell;
use std::rc::Rc;

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

use super::load_memory_errors::source;

/// Refuses exactly the `n`th growth (of any kind of block) after it is
/// armed, and keeps the bytes in use from what it allowed and was given
/// back.
#[derive(Default)]
struct Nth {
    left: Cell<Option<usize>>,
    refused: Cell<bool>,
    live: Cell<isize>,
}

struct RefuseNth(Rc<Nth>);

impl MemoryPolicy for RefuseNth {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        let k = &*self.0;
        if let Some(n) = k.left.get() {
            if n == 0 {
                k.left.set(None);
                k.refused.set(true);
                return false;
            }
            k.left.set(Some(n - 1));
        }
        k.live.set(k.live.get() + new as isize - old as isize);
        true
    }
    fn freed(&mut self, size: usize) {
        self.0.live.set(self.0.live.get() - size as isize);
    }
}

/// Every allocation of a load, the first, the second and so on, refused in
/// turn: each time the load fails with the memory error and leaves nothing
/// behind, and the vm goes on loading and running chunks. When the vm is
/// gone every block has been given back.
#[test]
fn a_load_refused_at_each_allocation_in_turn_leaks_nothing() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let nth = Rc::new(Nth::default());
        let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(RefuseNth(nth.clone()))));
        let src = source(v);
        let run = |vm: &mut Vm| {
            let cl = vm.load(src.as_bytes(), b"=chunk").unwrap();
            let r = vm.call_value(Value::Closure(cl), &[]).unwrap();
            assert!(matches!(r[0], Value::Str(_)), "{v:?}");
        };
        run(&mut vm);
        // refused loads and runs first: the collector's work lists keep the
        // room an emergency collection grew them to, and the compiler's and
        // the parser's vectors the room the last load needed, which is not
        // a leak
        for _ in 0..3 {
            nth.left.set(Some(0));
            let _ = vm.load(src.as_bytes(), b"=chunk");
            nth.left.set(None);
            run(&mut vm);
            vm.collect_garbage();
        }
        let base = nth.live.get();
        let mut n = 0;
        loop {
            nth.refused.set(false);
            nth.left.set(Some(n));
            let r = vm.load(src.as_bytes(), b"=chunk");
            nth.left.set(None);
            if !nth.refused.get() {
                assert!(r.is_ok(), "{v:?} n={n}");
                break;
            }
            // from 5.2 on the load collects in an emergency and runs once
            // more, which the single refusal lets through
            match r {
                Ok(_) => assert!(
                    v != LuaVersion::Lua51,
                    "{v:?} n={n}: a refused load succeeded"
                ),
                Err(e) => {
                    assert!(
                        e.is_memory(),
                        "{v:?} n={n}: {:?}",
                        String::from_utf8_lossy(&e.msg)
                    );
                    assert_eq!(e.msg, b"not enough memory", "{v:?} n={n}");
                }
            }
            // what the failed load held is given back by the next load,
            // which empties the vectors it left; a leak grows from one
            // failure to the next
            run(&mut vm);
            vm.collect_garbage();
            assert!(
                nth.live.get() <= base,
                "{v:?} n={n}: {} bytes in use after the failed load and a run, {base} before",
                nth.live.get()
            );
            n += 1;
            assert!(n < 200_000, "{v:?}: the load never succeeds");
        }
        assert!(n > 100, "{v:?}: only {n} allocations in a load");
        drop(vm);
        assert_eq!(nth.live.get(), 0, "{v:?}: blocks left after the vm is gone");
    }
}

/// `load` with a reader function that allocates, refused at each
/// allocation in turn: whether the refusal lands in the reader's Lua code,
/// in the parser or around them, the outcome is the memory error ("not
/// enough memory", returned by `load` or raised), never the end of the
/// process, and the vm goes on working.
#[test]
fn a_reader_refused_at_each_allocation_in_turn_gives_the_memory_error() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let nth = Rc::new(Nth::default());
        let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(RefuseNth(nth.clone()))));
        let chunk = vm
            .load(
                b"local parts = {}\n\
                  for i = 1, 40 do parts[i] = 'local x' .. i .. ' = {' .. i .. '}\\n' end\n\
                  parts[41] = 'return x40[1]'\n\
                  local i = 0\n\
                  local f, e = load(function()\n\
                    i = i + 1\n\
                    local p = parts[i]\n\
                    if p then return p .. string.rep(' ', i % 3) end\n\
                  end)\n\
                  if f then return f() end\n\
                  return nil, e\n",
                b"=reader",
            )
            .unwrap();
        // the collections below must not free it
        vm.set_global("reader_chunk", Value::Closure(chunk))
            .unwrap();
        let mut n = 0;
        let mut memory_errors = 0;
        loop {
            nth.refused.set(false);
            nth.left.set(Some(n));
            let r = vm.call_value(Value::Closure(chunk), &[]);
            nth.left.set(None);
            let refused = nth.refused.get();
            match r {
                Ok(vals)
                    if matches!(vals.first(), Some(Value::Int(40)))
                        || matches!(vals.first(), Some(Value::Float(x)) if *x == 40.0) =>
                {
                    if !refused {
                        break;
                    }
                }
                Ok(vals) => {
                    let Some(Value::Str(m)) = vals.get(1) else {
                        panic!("{v:?} n={n}: load gave {vals:?}");
                    };
                    assert_eq!(m.as_bytes(), b"not enough memory", "{v:?} n={n}");
                    memory_errors += 1;
                }
                Err(e) => {
                    let Value::Str(m) = e.0 else {
                        panic!("{v:?} n={n}: error {:?}", e.0)
                    };
                    assert_eq!(m.as_bytes(), b"not enough memory", "{v:?} n={n}");
                    memory_errors += 1;
                }
            }
            assert!(refused, "{v:?} n={n}: failed with nothing refused");
            vm.collect_garbage();
            n += 1;
            assert!(n < 200_000, "{v:?}: never succeeds");
        }
        // from 5.2 on most single refusals are retried after an emergency
        // collection and succeed
        let want = if v == LuaVersion::Lua51 { 50 } else { 20 };
        assert!(
            memory_errors > want,
            "{v:?}: only {memory_errors} memory errors"
        );
    }
}
