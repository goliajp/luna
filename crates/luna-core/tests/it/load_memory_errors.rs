//! Loading a chunk while the allocation context refuses memory: the
//! parser's and the compiler's vectors come from it, and a refusal is a
//! memory error from the load, never a crash.

use std::cell::Cell;
use std::rc::Rc;

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

/// What the test controls and sees: how many more growths of arrays and
/// buffers may succeed (`None`: all), and the most bytes in use so far.
#[derive(Default)]
struct Knobs {
    budget: Cell<Option<usize>>,
    peak: Cell<usize>,
}

struct Refuse(Rc<Knobs>);

impl MemoryPolicy for Refuse {
    fn allow(&mut self, old: usize, new: usize, kind: BlockKind, in_use: usize) -> bool {
        let k = &*self.0;
        k.peak.set(k.peak.get().max(in_use - old.min(in_use) + new));
        // objects are left alone: only the containers the load builds are
        // refused, so every refusal meets a path that reports it
        if kind != BlockKind::Other {
            return true;
        }
        match k.budget.get() {
            None => true,
            Some(0) => false,
            Some(n) => {
                k.budget.set(Some(n - 1));
                true
            }
        }
    }
}

/// A chunk with something of everything the compiler keeps: nested
/// functions, locals, upvalues, constants, long strings, loops, and gotos
/// where the dialect has them.
fn source(v: LuaVersion) -> String {
    let body = if v == LuaVersion::Lua51 {
        "for k = 1, 3 do if k ~= 2 then t.x = t.x + k end end"
    } else {
        "for k = 1, 3 do if k == 2 then goto skip end t.x = t.x + k ::skip:: end"
    };
    let mut s = String::new();
    for i in 0..60 {
        s.push_str(&format!(
            "local function f{i}(a, b, ...)\n  local t = {{x = a, y = b, '{long}', n = select('#', ...)}}\n  \
             {body}\n  \
             return function() return t.x .. '{long}' .. a, t.n end\nend\n",
            long = "s".repeat(50 + i)
        ));
    }
    s.push_str("return f59(1, 2)() \n");
    s
}

#[test]
fn every_refused_growth_during_a_load_is_a_memory_error() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let knobs = Rc::new(Knobs::default());
        let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(Refuse(knobs.clone()))));
        let src = source(v);
        let mut failures = 0;
        let mut n = 0;
        let cl = loop {
            knobs.budget.set(Some(n));
            match vm.load(src.as_bytes(), b"=chunk") {
                Ok(cl) => break cl,
                Err(e) => {
                    assert!(
                        e.is_memory(),
                        "{v:?} n={n}: {:?}",
                        String::from_utf8_lossy(&e.msg)
                    );
                    failures += 1;
                }
            }
            n += 1;
            assert!(n < 100_000, "{v:?}: the load never succeeds");
        };
        knobs.budget.set(None);
        assert!(failures > 10, "{v:?}: only {failures} refusals");
        let r = vm.call_value(Value::Closure(cl), &[]).unwrap();
        let Value::Str(s) = r[0] else {
            panic!("{v:?}: the chunk returns a string")
        };
        assert_eq!(
            s.as_bytes(),
            format!("5{}1", "s".repeat(109)).as_bytes(),
            "{v:?}"
        );
    }
}

#[test]
fn a_load_counts_what_the_parser_and_compiler_hold() {
    let knobs = Rc::new(Knobs::default());
    let mut vm = Vm::new_with_mem(
        LuaVersion::Lua54,
        MemOwner::policy(Box::new(Refuse(knobs.clone()))),
    );
    let mut src = String::new();
    for i in 0..4000 {
        src.push_str(&format!(
            "do local v{i} = {{a = {i}, b = v{i} or {i} + 1}} end\n",
            i = i % 150
        ));
    }
    let before = vm.memory_in_use().unwrap();
    knobs.peak.set(before);
    vm.load(src.as_bytes(), b"=big").unwrap();
    let grew = knobs.peak.get() - before;
    assert!(
        grew > src.len() / 2,
        "the load held {grew} bytes for {} of source",
        src.len()
    );
}

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
        // one refused load first: the collector's work lists keep the room
        // an emergency collection grew them to, which is not a leak
        nth.left.set(Some(0));
        let _ = vm.load(src.as_bytes(), b"=chunk");
        nth.left.set(None);
        run(&mut vm);
        vm.collect_garbage();
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
            vm.collect_garbage();
            assert!(
                nth.live.get() <= base,
                "{v:?} n={n}: {} bytes in use after the failed load, {base} before",
                nth.live.get()
            );
            run(&mut vm);
            vm.collect_garbage();
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
        assert!(
            memory_errors > 50,
            "{v:?}: only {memory_errors} memory errors"
        );
    }
}
