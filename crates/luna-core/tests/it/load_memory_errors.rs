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
