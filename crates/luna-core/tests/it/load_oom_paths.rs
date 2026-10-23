//! A load that runs out of memory inside a native, reached by each way the
//! interpreter calls a native: the memory error is the load's, and no
//! guard on the way (the fast loop's unguarded call, a protected call, a
//! hook's window, a metamethod, a coroutine) turns it into anything else.

use std::cell::Cell;
use std::rc::Rc;

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::{LuaError, Vm};

use super::load_memory_errors::source;
use luna_core::__private::VmCApiValues;

/// While armed, refuses every growth from the `n`th on.
#[derive(Default)]
struct FromNth {
    left: Cell<Option<usize>>,
    refused: Cell<bool>,
}

struct Refuse(Rc<FromNth>);

impl MemoryPolicy for Refuse {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        let k = &*self.0;
        match k.left.get() {
            None => true,
            Some(0) => {
                k.refused.set(true);
                false
            }
            Some(n) => {
                k.left.set(Some(n - 1));
                true
            }
        }
    }
}

std::thread_local! {
    static NATIVE_NTH: std::cell::RefCell<Option<Rc<FromNth>>> = const { std::cell::RefCell::new(None) };
}

/// Load a chunk with every allocation of the load from the `n`th on
/// refused: "ok" when nothing was refused, or the memory error's text.
fn load_nth(vm: &mut Vm, n: usize) -> &'static [u8] {
    let nth = NATIVE_NTH.with(|c| c.borrow().as_ref().unwrap().clone());
    let src = source(vm.version());
    nth.refused.set(false);
    nth.left.set(Some(n));
    let r = vm.load(src.as_bytes(), b"=inner");
    nth.left.set(None);
    match r {
        Ok(_) if nth.refused.get() => b"refused but loaded",
        Ok(_) => b"ok",
        Err(e) if e.is_memory() => b"not enough memory",
        Err(_) => b"other error",
    }
}

fn count_arg(v: Value) -> usize {
    match v {
        Value::Int(n) => n as usize,
        Value::Float(n) => n as usize,
        v => panic!("load_nth({v:?})"),
    }
}

/// `load_nth(n)`
fn nat_load_nth(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = count_arg(vm.nat_arg(fs, nargs, 0));
    let text = load_nth(vm, n);
    let s = vm.host_str(text)?;
    vm.nat_ret(fs, &[s])
}

/// `__index(t, n)`
fn nat_load_nth_mm(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = count_arg(vm.nat_arg(fs, nargs, 1));
    let text = load_nth(vm, n);
    let s = vm.host_str(text)?;
    vm.nat_ret(fs, &[s])
}

/// Run `body` (which sets `r` from `n`) in a loop over `n` until the load
/// succeeds, checking every failure is the memory error.
fn refused_until_ok(v: LuaVersion, prelude: &str, body: &str) {
    let nth = Rc::new(FromNth::default());
    NATIVE_NTH.with(|c| *c.borrow_mut() = Some(nth.clone()));
    let mut vm = Vm::new_with_mem(v, MemOwner::policy(Box::new(Refuse(nth.clone()))));
    let f = vm.native(nat_load_nth);
    vm.set_global("load_nth", f).unwrap();
    let f = vm.native(nat_load_nth_mm);
    vm.set_global("load_nth_mm", f).unwrap();
    let src = format!(
        "{prelude}\n\
         local mem, ok = 0, 0\n\
         for n = 0, 100000 do\n\
           local r\n\
           {body}\n\
           if r == 'ok' then ok = 1 break end\n\
           assert(r == 'not enough memory', r)\n\
           mem = mem + 1\n\
         end\n\
         return mem, ok\n"
    );
    let cl = vm.load(src.as_bytes(), b"=loop").unwrap();
    let r = vm
        .call_value(Value::Closure(cl), &[])
        .unwrap_or_else(|e| match e.0 {
            Value::Str(s) => panic!("{v:?} {body}: {}", String::from_utf8_lossy(s.as_bytes())),
            x => panic!("{v:?} {body}: {x:?}"),
        });
    let num = |x: Value| match x {
        Value::Int(i) => i,
        Value::Float(f) => f as i64,
        _ => panic!("{v:?} {body}: {r:?}"),
    };
    let (mem, ok) = (num(r[0]), num(r[1]));
    assert_eq!(ok, 1, "{v:?} {body}: the load never succeeded");
    assert!(mem > 100, "{v:?} {body}: only {mem} memory errors");
}

const DIALECTS: [LuaVersion; 3] = [LuaVersion::Lua51, LuaVersion::Lua54, LuaVersion::Lua55];

/// The fast loop calls a plain native with no guard of its own.
#[test]
fn a_load_inside_a_native_called_from_a_loop_gives_the_memory_error() {
    for v in DIALECTS {
        refused_until_ok(v, "", "r = load_nth(n)");
    }
}

/// Under `pcall` the native runs inside the guarded call path.
#[test]
fn a_load_inside_a_protected_native_gives_the_memory_error() {
    for v in DIALECTS {
        refused_until_ok(
            v,
            "",
            "local good good, r = pcall(load_nth, n) assert(good, r)",
        );
    }
}

/// A call hook runs in the native's window before it is entered, and
/// takes the loop off the fast path.
#[test]
fn a_load_inside_a_hooked_native_gives_the_memory_error() {
    for v in DIALECTS {
        refused_until_ok(v, "debug.sethook(function() end, 'cr')", "r = load_nth(n)");
    }
}

/// A native `__index` runs as a metamethod call.
#[test]
fn a_load_inside_a_native_metamethod_gives_the_memory_error() {
    for v in DIALECTS {
        refused_until_ok(
            v,
            "local t = setmetatable({}, {__index = load_nth_mm})",
            "r = t[n]",
        );
    }
}

/// Inside a coroutine the native runs under the resume's own loop.
#[test]
fn a_load_inside_a_native_in_a_coroutine_gives_the_memory_error() {
    for v in DIALECTS {
        refused_until_ok(
            v,
            "",
            "r = coroutine.wrap(function(k) return load_nth(k) end)(n)",
        );
    }
}
