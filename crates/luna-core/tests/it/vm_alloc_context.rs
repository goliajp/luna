//! A Vm whose memory comes from a host allocation function or a memory
//! policy: what the function sees, and what the Vm reports.

use std::alloc::Layout;
use std::cell::Cell;
use std::ffi::c_void;
use std::rc::Rc;

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[derive(Default)]
struct Seen {
    live: Cell<usize>,
    news: Cell<usize>,
    /// new blocks by the `osize` they came with
    kinds: [Cell<usize>; 12],
}

unsafe extern "C" fn counting(
    ud: *mut c_void,
    p: *mut c_void,
    os: usize,
    ns: usize,
) -> *mut c_void {
    // SAFETY: every test passes a live `Seen` as `ud`
    let s = unsafe { &*(ud as *const Seen) };
    let old = if p.is_null() { 0 } else { os };
    if ns == 0 {
        if !p.is_null() {
            // SAFETY: `p` is a live block of `os` bytes this function made
            // with alignment 16
            unsafe { std::alloc::dealloc(p.cast(), Layout::from_size_align(os, 16).unwrap()) };
            s.live.set(s.live.get() - os);
        }
        return std::ptr::null_mut();
    }
    let q = if p.is_null() {
        s.news.set(s.news.get() + 1);
        s.kinds[os.min(11)].set(s.kinds[os.min(11)].get() + 1);
        // SAFETY: the size is not 0
        unsafe { std::alloc::alloc(Layout::from_size_align(ns, 16).unwrap()) }
    } else {
        // SAFETY: `p` is a live block of `os` bytes this function made with
        // alignment 16; the new size is not 0
        unsafe { std::alloc::realloc(p.cast(), Layout::from_size_align(os, 16).unwrap(), ns) }
    };
    assert!(!q.is_null());
    s.live.set(s.live.get() - old + ns);
    q.cast()
}

fn raw_vm(s: &Seen, v: LuaVersion) -> Vm {
    // SAFETY: `counting` follows the `lua_Alloc` contract, and every test
    // drops the Vm before `s`
    let mem = unsafe { MemOwner::raw(counting, s as *const Seen as *mut c_void, v) };
    Vm::new_with_mem(v, mem)
}

const SCRIPT: &str = r#"
    local t = {}
    for i = 1, 200 do t[i] = {name = "item" .. i, f = function() return i end} end
    local co = coroutine.create(function(x) coroutine.yield(x .. "!") end)
    coroutine.resume(co, string.rep("ab", 100))
    return #t, collectgarbage("count")
"#;

#[test]
fn objects_come_from_the_host_function_and_go_back_to_it() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let s = Seen::default();
        {
            let mut vm = raw_vm(&s, v);
            let r = vm.eval(SCRIPT).unwrap();
            assert!(matches!(r[0], Value::Int(200)), "{v:?}");
            let in_use = vm.memory_in_use().expect("a host function counts");
            assert_eq!(in_use, s.live.get(), "{v:?}");
            let Value::Float(kb) = r[1] else {
                panic!("count is a float")
            };
            assert!(kb > 0.0, "{v:?} {kb}");
            if v == LuaVersion::Lua51 {
                assert_eq!(
                    s.kinds[0].get(),
                    s.news.get(),
                    "5.1 passes 0 for every new block"
                );
            } else {
                for (code, what) in [(4, "string"), (5, "table"), (6, "function"), (8, "thread")] {
                    assert!(s.kinds[code].get() > 0, "{v:?}: no {what} block");
                }
            }
        }
        assert_eq!(s.live.get(), 0, "{v:?}: every block is freed with the Vm");
    }
}

/// How many growths a policy was asked about, and the largest one.
struct Watch(Rc<(Cell<usize>, Cell<usize>)>);

impl MemoryPolicy for Watch {
    fn allow(&mut self, old: usize, new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        let (asked, biggest) = &*self.0;
        asked.set(asked.get() + 1);
        biggest.set(biggest.get().max(new - old.min(new)));
        true
    }
}

#[test]
fn a_policy_sees_every_growth_and_the_vm_counts_it() {
    let seen = Rc::new((Cell::new(0), Cell::new(0)));
    let mem = MemOwner::policy(Box::new(Watch(seen.clone())));
    let mut vm = Vm::new_with_mem(LuaVersion::Lua54, mem);
    let before = vm.memory_in_use().unwrap();
    vm.eval("s = string.rep('x', 100000)").unwrap();
    assert!(vm.memory_in_use().unwrap() >= before + 100000);
    assert!(seen.1.get() >= 100000);
    assert!(seen.0.get() > 0);
}

#[test]
fn the_system_allocator_is_not_counted() {
    let vm = Vm::new(LuaVersion::Lua54);
    assert_eq!(vm.memory_in_use(), None);
}
