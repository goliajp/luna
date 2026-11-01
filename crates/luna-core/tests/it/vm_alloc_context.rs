//! A Vm whose memory comes from a host allocation function or a memory
//! policy: what the function sees, and what the Vm reports.

use crate::sync_cell::SyncCell;
use std::alloc::Layout;
use std::ffi::c_void;
use std::sync::Arc;

use luna_core::runtime::Value;
use luna_core::runtime::mem::{BlockKind, MemOwner, MemoryPolicy};
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

#[derive(Default)]
struct Seen {
    live: SyncCell<usize>,
    news: SyncCell<usize>,
    /// new blocks by the `osize` they came with
    kinds: [SyncCell<usize>; 12],
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
            s.live.set(s.live.get().wrapping_sub(os));
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
    s.live.set(s.live.get().wrapping_sub(old).wrapping_add(ns));
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
struct Watch(Arc<(SyncCell<usize>, SyncCell<usize>)>);

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
    let seen = Arc::new((SyncCell::new(0), SyncCell::new(0)));
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

#[test]
fn vms_given_clones_of_one_owner_share_its_count() {
    let o = MemOwner::policy(Box::new(luna_core::runtime::mem::MemoryLimit(usize::MAX)));
    let a = Vm::new_with_mem(LuaVersion::Lua54, o.clone());
    let one = a.memory_in_use().expect("a policy counts");
    let mut b = Vm::new_with_mem(LuaVersion::Lua54, o);
    let two = b.memory_in_use().expect("a policy counts");
    assert!(two > one, "{two} after the second Vm, {one} before");
    assert_eq!(a.memory_in_use(), Some(two));
    b.eval("keep = string.rep('x', 100000)").unwrap();
    assert!(a.memory_in_use().unwrap() >= two + 100000);
    drop(b);
    assert_eq!(a.memory_in_use(), Some(one));
}

/// Panics the first time it is asked after `armed`.
struct PanicOnce(Arc<SyncCell<bool>>);

impl MemoryPolicy for PanicOnce {
    fn allow(&mut self, _old: usize, _new: usize, _kind: BlockKind, _in_use: usize) -> bool {
        if self.0.get() {
            self.0.set(false);
            panic!("the policy gives up once");
        }
        true
    }
}

// with or without the `send` feature, a policy that panicked is asked again
// for the next block (a lock it left poisoned does not fail every later
// allocation, nor the frees as the Vm drops)
#[test]
fn a_policy_that_panicked_once_is_asked_again() {
    let armed = Arc::new(SyncCell::new(false));
    let mem = MemOwner::policy(Box::new(PanicOnce(armed.clone())));
    let mut vm = Vm::new_with_mem(LuaVersion::Lua54, mem.clone());
    armed.set(true);
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        vm.eval("local t = {} for i = 1, 100 do t[i] = {i} end")
    }));
    assert!(r.is_err(), "the policy's panic reaches the caller");
    drop(vm);
    let mut vm = Vm::new_with_mem(LuaVersion::Lua54, mem);
    let r = vm.eval("local t = {} for i = 1, 100 do t[i] = {i} end return #t");
    assert!(matches!(r.unwrap()[0], Value::Int(100)));
}
